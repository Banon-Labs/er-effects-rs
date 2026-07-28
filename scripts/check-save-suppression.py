#!/usr/bin/env python3
"""Single verdict for a save-census / save-suppression run.

Two independent oracles have to agree before anything is claimed:

  in-process  er-save-disable-telemetry.json  -- which call sites reached save data
  offline     save-write-witness snapshots    -- whether the bytes on disk moved

Neither alone is trustworthy. The in-process census can only see writes that go
through the APIs it hooked, so on its own a zero reading is indistinguishable from
"the DLL was blind". The offline witness sees every byte that landed but knows
nothing about who wrote it. Cross-checking them turns each one's blind spot into a
detectable contradiction:

  census says writes happened  + file changed      -> consistent (census is working)
  census says no writes        + file unchanged    -> consistent
  census says no writes        + file CHANGED      -> COVERAGE GAP: the game reached
                                                      disk by a route the census does
                                                      not hook. This is the failure
                                                      the census exists to catch, and
                                                      it must never be reported as a
                                                      clean run.
  census says writes happened  + file unchanged    -> writes were attempted and did
                                                      not land: expected once
                                                      suppression is active.

Usage::

    python3 scripts/check-save-suppression.py \\
        --telemetry "$GAME_DIR/er-save-disable-telemetry.json" \\
        --before before.json --after after.json

Exit code 0 means the run's verdict is PASS, 1 means FAIL, 2 means the inputs were
not usable (which is itself never a pass).
"""
from __future__ import annotations

import argparse
import json
import subprocess
import sys
from pathlib import Path
from typing import Any

REPO_ROOT = Path(__file__).resolve().parents[1]
WITNESS = REPO_ROOT / "scripts" / "save-write-witness.py"
# Bounded per the repo's 30s cap on non-game operations; the witness diff is a
# pure in-memory comparison of two JSON files and finishes in well under a second.
WITNESS_TIMEOUT_SECONDS = 20.0

PHASE_CENSUS = "census-only"


def load_json(path: Path) -> dict[str, Any] | None:
    try:
        return json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError):
        return None


def offline_diff(before: Path, after: Path) -> dict[str, Any] | None:
    """Delegate to the witness so there is exactly one diff implementation."""
    try:
        completed = subprocess.run(
            [sys.executable, str(WITNESS), "diff", str(before), str(after), "--json"],
            capture_output=True,
            text=True,
            timeout=WITNESS_TIMEOUT_SECONDS,
            check=False,
        )
    except (OSError, subprocess.TimeoutExpired):
        return None
    try:
        return json.loads(completed.stdout)
    except json.JSONDecodeError:
        return None


def evaluate(telemetry: dict[str, Any] | None, diff: dict[str, Any] | None) -> tuple[bool, str, list[str]]:
    """Return (passed, verdict, reasons)."""
    reasons: list[str] = []

    if telemetry is None:
        return (
            False,
            "FAIL -- no in-process census",
            [
                "The telemetry file is missing or unparseable, so the DLL never installed.",
                "An absent census is NOT a clean run: nothing was watching.",
            ],
        )

    phase = telemetry.get("phase", "unknown")
    installed = telemetry.get("census_hooks_installed", 0)
    expected = telemetry.get("census_hooks_expected", 0)
    escaped = telemetry.get("escaped_write_sites", []) or []

    if installed < expected:
        reasons.append(
            f"Census coverage is incomplete: {installed}/{expected} file APIs hooked. "
            "A save could have used an unhooked API without being seen."
        )

    if diff is None:
        return (
            False,
            "FAIL -- no offline witness",
            reasons + ["Could not compute the before/after save-file diff."],
        )

    file_changed = not diff.get("clean", False)
    census_saw_writes = bool(escaped)

    # The contradiction that matters most: bytes landed but nothing was observed.
    if file_changed and not census_saw_writes:
        changed = diff.get("content_changes", []) + diff.get("mtime_only_changes", [])
        reasons.append(
            "COVERAGE GAP: the save file changed on disk but the census recorded zero "
            "write sites. The game reached the filesystem by a route the census does "
            "not hook (CreateFile2 / NtWriteFile / a mapped section are the leads)."
        )
        reasons.extend(f"  changed: {entry['path']}: {entry['reason']}" for entry in changed)
        return False, "FAIL -- census blind spot", reasons

    if phase == PHASE_CENSUS:
        # Observation-only build. It suppresses nothing, so a changed save file is the
        # expected outcome; the deliverable is the call-site inventory, and the run is
        # only useful if it actually observed something.
        if not census_saw_writes:
            reasons.append(
                "No save writes were observed. Either no save was triggered during the "
                "run, or the census is blind. The offline witness agrees the file did "
                "not change, so the most likely explanation is that no save happened -- "
                "trigger one (rest at a grace, or quit to title) and re-run."
            )
            return False, "INCONCLUSIVE -- census observed nothing", reasons
        reasons.append(f"Observed {len(escaped)} distinct save-write call site(s):")
        for site in escaped:
            rvas = ", ".join(site.get("game_rvas", [])) or "no game frames captured"
            reasons.append(
                f"  {site.get('api')} x{site.get('hits')} ({site.get('bytes')} bytes) -> {site.get('path')}"
            )
            reasons.append(f"    game RVAs: {rvas}")
        reasons.append(
            "This is a CENSUS run: saving was not suppressed, so this verdict says the "
            "inventory is real, not that saving is disabled."
        )
        return True, "PASS -- census captured the save call sites", reasons

    # Suppression build: both oracles must be clean.
    if census_saw_writes:
        reasons.append(f"{len(escaped)} save-write call site(s) escaped suppression:")
        for site in escaped:
            rvas = ", ".join(site.get("game_rvas", [])) or "no game frames captured"
            reasons.append(f"  {site.get('api')} x{site.get('hits')} -> {site.get('path')} [{rvas}]")
    if file_changed:
        reasons.append("The save file on disk changed; suppression did not hold.")

    if census_saw_writes or file_changed:
        return False, "FAIL -- saving was not fully suppressed", reasons

    reasons.append("No call site reached save data and no save byte changed on disk.")
    return True, "PASS -- saving fully suppressed", reasons


def selftest() -> int:
    """Verify each verdict branch, especially that a blind census cannot pass."""
    failures: list[str] = []

    def check(name: str, telemetry: Any, diff: Any, want_pass: bool, want_in_verdict: str) -> None:
        passed, verdict, _ = evaluate(telemetry, diff)
        if passed != want_pass or want_in_verdict.lower() not in verdict.lower():
            failures.append(f"{name}: got ({passed}, {verdict!r}), wanted ({want_pass}, ~{want_in_verdict!r})")

    census_site = {"api": "WriteFile", "hits": 3, "bytes": 2600000, "path": "ER0000.sl2", "game_rvas": ["0x67a050"]}
    full_census = {
        "phase": PHASE_CENSUS,
        "census_hooks_installed": 6,
        "census_hooks_expected": 6,
        "escaped_write_sites": [census_site],
    }
    blind_census = dict(full_census, escaped_write_sites=[])
    suppressing = dict(full_census, phase="suppress", escaped_write_sites=[])
    leaking = dict(full_census, phase="suppress")
    dirty = {"clean": False, "content_changes": [{"path": "ER0000.sl2", "reason": "contents changed"}], "mtime_only_changes": []}
    cleanfile = {"clean": True, "content_changes": [], "mtime_only_changes": []}

    check("missing telemetry", None, cleanfile, False, "no in-process census")
    check("missing diff", full_census, None, False, "no offline witness")
    # The critical one: bytes landed, census saw nothing. Must never pass.
    check("blind census", blind_census, dirty, False, "blind spot")
    check("census observed writes", full_census, dirty, True, "census captured")
    check("census saw nothing, file clean", blind_census, cleanfile, False, "inconclusive")
    check("suppression holding", suppressing, cleanfile, True, "fully suppressed")
    check("suppression leaking", leaking, cleanfile, False, "not fully suppressed")
    check("suppression clean census but file changed", suppressing, dirty, False, "blind spot")

    # A census-only run must never be reported as suppression working.
    _, verdict, _ = evaluate(full_census, dirty)
    if "suppress" in verdict.lower():
        failures.append("census-only run reported as suppression")

    if failures:
        for failure in failures:
            print(f"SELFTEST FAIL: {failure}", file=sys.stderr)
        return 1
    print("SELFTEST PASS: blind-census, coverage-gap, and phase confusion all rejected")
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--telemetry", type=Path, help="er-save-disable-telemetry.json from the run")
    parser.add_argument("--before", type=Path, help="save-write-witness snapshot taken before the run")
    parser.add_argument("--after", type=Path, help="save-write-witness snapshot taken after the run")
    parser.add_argument("--selftest", action="store_true")
    args = parser.parse_args()

    if args.selftest:
        return selftest()

    if not (args.telemetry and args.before and args.after):
        parser.error("--telemetry, --before and --after are all required")

    telemetry = load_json(args.telemetry)
    diff = offline_diff(args.before, args.after)

    passed, verdict, reasons = evaluate(telemetry, diff)
    print(verdict)
    for reason in reasons:
        print(f"  {reason}")
    return 0 if passed else 1


if __name__ == "__main__":
    raise SystemExit(main())
