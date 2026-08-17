#!/usr/bin/env python3
"""Wait for a launched run to end, then remove exactly what it staged.

Runs DETACHED from whatever launched it (own session, no controlling terminal), because the
game routinely outlives the shell -- and, in an agent session, the turn -- that started it. A
reaper tied to that lifetime would be killed at the moment it became useful.

It is the FAST path, not the guarantee. It can still be SIGKILLed, the machine can reboot, and
`scripts/er-stale-run-sentinel.sh` can tear the game down from a PostToolUse hook without ever
telling it. The guarantee is that `er-run-branch.py` garbage-collects every dead run's state
before staging a new one, so a leftover survives at most until the next launch. Both paths call
the same idempotent `RunState.cleanup()`.

Waiting is event-driven: a pidfd via `select`, never a poll loop with sleeps.

Usage (normally spawned by er-run-branch.py, not by hand):
    python3 scripts/er-run-reaper.py --run-id <id> [--place-monitor <name>]
    python3 scripts/er-run-reaper.py --selftest
"""

from __future__ import annotations

import argparse
import subprocess
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

# Imported as a MODULE, not by-value: the selftest rebinds `er_run_lib.RUN_STATE_ROOT` to a
# temp dir, and a from-import would keep pointing at the real one and clean the user's runs.
import er_run_lib  # noqa: E402

RunState = er_run_lib.RunState
find_game_pids = er_run_lib.find_game_pids
process_alive = er_run_lib.process_alive
wait_for_exit = er_run_lib.wait_for_exit

REPO_ROOT = Path(__file__).resolve().parent.parent
PLACER = REPO_ROOT / "scripts" / "place-er-window-hyprland.py"

# One bounded wait is re-armed in a loop rather than asking for one long one, so the reaper
# stays responsive and every individual wait honours the repo's 30s ceiling.
WAIT_SLICE_SECONDS = 25.0
# The ER window appears long after the process does. Give placement a bounded number of
# slices, then give up quietly -- window placement is a convenience, never a gate.
PLACEMENT_SLICES = 8


def place_window(monitor: str) -> None:
    """Best-effort move of the ER window. Class-filtered: it never enumerates other windows."""
    if not PLACER.is_file():
        return
    try:
        subprocess.run(
            [sys.executable, str(PLACER), "--monitor", monitor],
            timeout=20,
            capture_output=True,
            check=False,
        )
    except (OSError, subprocess.TimeoutExpired):
        pass


def reap(run_id: str, monitor: str | None) -> int:
    state = RunState.load(er_run_lib.RUN_STATE_ROOT / run_id / "run.json")
    if state is None:
        return 1

    placed = False
    slices = 0
    while True:
        if monitor and not placed and slices < PLACEMENT_SLICES:
            if find_game_pids():
                place_window(monitor)
                placed = True

        if state.pid and process_alive(state.pid):
            wait_for_exit(state.pid, WAIT_SLICE_SECONDS)
            slices += 1
            continue

        # The launcher is gone. me3 normally outlives the game, but if it exited first the
        # game would still be up -- cleaning now would delete a profile still in use.
        remaining = find_game_pids()
        if remaining:
            wait_for_exit(remaining[0], WAIT_SLICE_SECONDS)
            slices += 1
            continue
        break

    state.cleanup()
    return 0


def selftest() -> int:
    ok = True

    def check(condition: bool, label: str) -> None:
        nonlocal ok
        if not condition:
            ok = False
        print(("  ok   " if condition else "  FAIL ") + label)

    import tempfile

    check(PLACER.is_file(), f"the window placer this delegates to exists ({PLACER.name})")
    check(
        isinstance(find_game_pids(), list),
        "the game-process scan reads /proc and returns a list (no pgrep involved)",
    )

    with tempfile.TemporaryDirectory() as raw:
        root = Path(raw)
        staged = root / "run.me3"
        staged.write_text("x", encoding="utf-8")
        previous, er_run_lib.RUN_STATE_ROOT = er_run_lib.RUN_STATE_ROOT, root
        try:
            child = subprocess.Popen(["/bin/true"])
            child.wait()
            state = er_run_lib.RunState(
                run_id="reaper-selftest",
                pid=child.pid,
                profile=str(staged),
                remove_paths=[str(staged)],
            )
            state.save()

            code = reap("reaper-selftest", monitor=None)
            check(code == 0, "reaping an already-exited run succeeds")
            check(not staged.exists(), "the staged profile is removed")
            check(
                not (root / "reaper-selftest").exists(),
                "the run-state directory is removed too, so GC has nothing left to find",
            )
            check(reap("reaper-selftest", monitor=None) == 1, "reaping an unknown run is a no-op")
        finally:
            er_run_lib.RUN_STATE_ROOT = previous

    print("selftest:", "PASS" if ok else "FAIL")
    return 0 if ok else 1


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--run-id")
    parser.add_argument("--place-monitor", help="Hyprland monitor name to move the ER window to")
    parser.add_argument("--selftest", action="store_true")
    args = parser.parse_args()

    if args.selftest:
        return selftest()
    if not args.run_id:
        parser.error("--run-id is required")
    return reap(args.run_id, args.place_monitor)


if __name__ == "__main__":
    sys.exit(main())
