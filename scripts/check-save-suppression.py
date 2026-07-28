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
import re
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
# The only other value `lib.rs::phase()` emits. Fixtures below must use these two
# literals and nothing else: a fixture phase the DLL never produces silently exercises
# whichever branch it happens to fall through to.
PHASE_SUPPRESS = "suppress+census"


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


def changed_slot_bytes(diff: dict[str, Any]) -> tuple[int, list[str]]:
    """Total bytes of BND4 entries the offline diff says changed, and their names.

    Slot sizes are parsed out of the witness's per-slot lines ("slot USER_DATA000:
    contents changed (2621456B -> 2621456B)"). Only the primary container is counted;
    a `.bak` mirrors it and would double the total.
    """
    total = 0
    names: list[str] = []
    for change in diff.get("content_changes", []):
        if ".bak" in change.get("path", ""):
            continue
        for slot in change.get("slots", []):
            match = re.search(r"slot (\S+): contents changed \((\d+)B", slot)
            if match:
                names.append(match.group(1))
                total += int(match.group(2))
    return total, names


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

    # The census seeing SOMETHING is not the same as it seeing EVERYTHING. Compare how many
    # bytes the census accounts for against how many actually moved on disk. This check exists
    # because the presence/absence test below reported PASS on a run where the census logged a
    # single 2359328-byte write while the offline diff found three changed slots totalling ~5.4MB
    # -- the missing bytes went out through CopyFileW, which was not hooked.
    changed_bytes, changed_names = changed_slot_bytes(diff)
    # Prefer the total that includes copies; fall back for telemetry from an older build
    # (which under-counts, so the gap check simply fires more readily there).
    census_bytes = telemetry.get("total_bytes_to_disk") or telemetry.get("write_bytes", 0) or 0
    if changed_bytes > census_bytes:
        reasons.append(
            f"COVERAGE GAP: {changed_bytes} bytes changed on disk across slots "
            f"{', '.join(changed_names)}, but the census only accounts for {census_bytes} bytes. "
            f"{changed_bytes - census_bytes} bytes reached the filesystem by a route the census "
            "does not hook."
        )
        return False, "FAIL -- census under-counted the writes", reasons

    # Suppression-specific gates. A file-IO census cannot answer either of these, and
    # letting it imply them is how a run gets called PASS on evidence it does not have.
    if phase != PHASE_CENSUS:
        swallowed = telemetry.get("suppress_submits_swallowed", 0) or 0
        passed = telemetry.get("suppress_submits_passed_through", 0) or 0
        mismatches = telemetry.get("suppress_prologue_mismatches", 0) or 0
        bc4_max = telemetry.get("quit_phase_bc4_max_seen")

        if mismatches:
            reasons.append(
                f"{mismatches} hooked address(es) failed their prologue byte check -- the "
                "suppression was installed against code that is not what it claims to be."
            )
            return False, "FAIL -- hook target drift", reasons
        if passed:
            reasons.append(f"{passed} save submit(s) were NOT swallowed and reached the IO queue.")
            return False, "FAIL -- suppression leaked", reasons
        if not swallowed:
            reasons.append(
                "Suppression was armed but swallowed zero submits, so an unchanged save file "
                "proves nothing -- nothing tried to save. Trigger a save and re-run."
            )
            return False, "INCONCLUSIVE -- nothing was suppressed", reasons
        settle_events = telemetry.get("quit_phase_settle_events")
        if settle_events:
            reasons.append(
                f"Quit path released {settle_events} time(s): the bc4 2->3 transition "
                "executed, so the System->Quit wait job was satisfied rather than spinning."
            )
        elif settle_events == 0 and telemetry.get("quit_settle_observer_installed") is False:
            reasons.append(
                "The quit-settle observer did not install, so quit_phase_settle_events is "
                "structurally 0 and says nothing about the quit path. Deadlock-freedom is "
                "UNPROVEN for this run, not disproven."
            )
        elif settle_events == 0 and bc4_max and bc4_max >= 2:
            reasons.append(
                "A quit was requested (bc4 reached 2) but the 2->3 transition never fired: "
                "the System->Quit wait job would spin forever."
            )
            return False, "FAIL -- quit path deadlocked", reasons
        elif bc4_max is None:
            reasons.append(
                "No quit-phase reading. This build cannot show that quit-to-title survives "
                "suppression; deadlock-freedom is UNPROVEN, not proven."
            )
        elif bc4_max >= 3:
            reasons.append(
                f"Quit phase reached {bc4_max}: GameMan+0xbc4 settled, so the System->Quit "
                "wait job was released rather than spinning."
            )
        elif bc4_max > 0:
            # NOT a failure on its own. bc4 == 3 is transient: it is set, consumed by the
            # wait job, and reset to 0 within the same quit sequence, so a sampled maximum
            # of 2 is the NORMAL reading even on a quit that worked perfectly. Two runs
            # with a user-confirmed working quit both read 2 here. The event counter above
            # is the authority.
            reasons.append(
                f"Sampled quit phase peaked at {bc4_max}; bc4 == 3 is transient so this is "
                "not evidence either way. See quit_phase_settle_events."
            )

    # Blindness gates, applied to BOTH phases.
    #
    # Each is a verified false-PASS path if left ungated: the counter moves, the real file
    # is unchanged, `escaped_write_sites` stays empty, and the verdict reads clean for a
    # run that never actually observed anything.
    #
    # A `diverted_open_failures` gate used to sit here too, for a path-diversion layer that
    # could hand the game an invalid handle and make its save genuinely fail while the
    # verdict read clean. That layer was deleted rather than gated: suppression never opens
    # a save file at all, so the failure it guarded cannot occur.
    blind = (telemetry.get("sites_dropped", 0) or 0) + (telemetry.get("handles_dropped", 0) or 0)
    if blind:
        reasons.append(
            f"The census went blind ({telemetry.get('sites_dropped', 0)} site(s) and "
            f"{telemetry.get('handles_dropped', 0)} handle(s) dropped at their table caps). "
            "From that point an escaped write could not be recorded, so an empty "
            "escaped_write_sites proves nothing."
        )
        return False, "INCONCLUSIVE -- census went blind", reasons

    publish_failures = telemetry.get("publish_failures", 0) or 0
    if publish_failures:
        reasons.append(
            f"{publish_failures} telemetry publish(es) failed to land, so the counters "
            "read here may be stale."
        )
        return False, "INCONCLUSIVE -- telemetry may be stale", reasons

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
        # Must track hooks::EXPECTED_HOOKS; 6 was stale and let a coverage-gap fixture
        # look fully hooked.
        "census_hooks_installed": 8,
        "census_hooks_expected": 8,
        # Accounts for every byte the `dirty` fixture says moved -- a fully-sighted census.
        "write_bytes": 2359328,
        "escaped_write_sites": [census_site],
    }
    blind_census = dict(full_census, escaped_write_sites=[])
    suppressing = dict(full_census, phase=PHASE_SUPPRESS, escaped_write_sites=[],
                       suppress_submits_swallowed=25, quit_phase_bc4_max_seen=3)
    leaking = dict(full_census, phase=PHASE_SUPPRESS,
                   suppress_submits_swallowed=25, quit_phase_bc4_max_seen=3)
    dirty = {
        "clean": False,
        "content_changes": [
            {
                "path": "ER0000.sl2",
                "reason": "contents changed",
                "slots": ["slot USER_DATA011: contents changed (2359328B -> 2359328B)"],
            }
        ],
        "mtime_only_changes": [],
    }
    cleanfile = {"clean": True, "content_changes": [], "mtime_only_changes": []}
    # The real regression: three slots moved, the census logged only the smallest of them.
    under_counted = {
        "clean": False,
        "content_changes": [
            {
                "path": "ER0000.sl2",
                "reason": "contents changed",
                "slots": [
                    "slot USER_DATA000: contents changed (2621456B -> 2621456B)",
                    "slot USER_DATA010: contents changed (393232B -> 393232B)",
                    "slot USER_DATA011: contents changed (2359328B -> 2359328B)",
                ],
            },
            {
                "path": "ER0000.sl2.bak",
                "reason": "contents changed",
                "slots": ["slot USER_DATA000: contents changed (2621456B -> 2621456B)"],
            },
        ],
        "mtime_only_changes": [],
    }

    check("missing telemetry", None, cleanfile, False, "no in-process census")
    check("missing diff", full_census, None, False, "no offline witness")
    # The critical one: bytes landed, census saw nothing. Must never pass.
    check("blind census", blind_census, dirty, False, "blind spot")
    check("census observed writes", full_census, dirty, True, "census captured")
    # Seeing SOME writes must not excuse missing most of them.
    check("census under-counted", full_census, under_counted, False, "under-counted")
    # The .bak mirror must not be double-counted into a false shortfall.
    counted, names = changed_slot_bytes(under_counted)
    if counted != 2621456 + 393232 + 2359328 or "USER_DATA000" not in names:
        failures.append(f"changed_slot_bytes miscounted: {counted} {names}")
    check("census saw nothing, file clean", blind_census, cleanfile, False, "inconclusive")
    check("suppression holding", suppressing, cleanfile, True, "fully suppressed")
    # Suppression-specific gates: each must fail on its own evidence, not on file IO.
    armed = dict(suppressing, suppress_submits_swallowed=25, quit_phase_bc4_max_seen=3)
    check("armed, saves swallowed, quit settled", armed, cleanfile, True, "fully suppressed")
    check("armed but nothing swallowed", dict(armed, suppress_submits_swallowed=0),
          cleanfile, False, "inconclusive")
    check("a submit leaked past suppression", dict(armed, suppress_submits_passed_through=1),
          cleanfile, False, "leaked")
    check("hook target drift", dict(armed, suppress_prologue_mismatches=1),
          cleanfile, False, "drift")
    # The one a file-IO census can never catch: everything on disk is clean and the
    # quit path is wedged.
    # The transient-value trap: a sampled max of 2 with the settle event present is a
    # PASS, because bc4 == 3 never survives long enough to be sampled.
    check("sampled 2 but settle event fired", dict(armed, quit_phase_bc4_max_seen=2,
          quit_phase_settle_events=1), cleanfile, True, "fully suppressed")
    check("quit requested but never settled", dict(armed, quit_phase_bc4_max_seen=2,
          quit_phase_settle_events=0), cleanfile, False, "deadlock")
    check("suppression leaking", leaking, cleanfile, False, "not fully suppressed")
    check("suppression clean census but file changed", suppressing, dirty, False, "blind spot")

    # Verified false-PASS paths. Each looks IDENTICAL to a perfect run from the file's
    # point of view -- clean diff, empty escaped_write_sites -- so only the counter
    # distinguishes them, and before these gates existed all of them returned PASS.
    check("site table overflowed, census blind", dict(armed, sites_dropped=1),
          cleanfile, False, "blind")
    check("handle table overflowed, census blind", dict(armed, handles_dropped=1),
          cleanfile, False, "blind")
    check("telemetry publish did not land", dict(armed, publish_failures=1),
          cleanfile, False, "stale")

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
