#!/usr/bin/env python3
"""Regression tests for scripts/semaphore_watchdog.py.

Pure-logic tests: we drive the watchdog with synthetic telemetry samples at explicit monotonic
times, so they run without a game and pin the behaviors that make the model correct -- especially
the two that are easy to get wrong: (1) permille RESETTING between the first and second load must
not read as a stall, and (2) a liveness counter (present_hook_hits) that keeps ticking while the
world is wedged at WORLD RES WAIT must NOT keep the run alive.
"""
from __future__ import annotations

import importlib.util
import sys
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[1]
WATCHDOG_PATH = REPO_ROOT / "scripts" / "semaphore_watchdog.py"


def load_mod():
    spec = importlib.util.spec_from_file_location("semaphore_watchdog", WATCHDOG_PATH)
    if spec is None or spec.loader is None:
        raise RuntimeError(f"failed to load {WATCHDOG_PATH}")
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def check_coerce_number(m) -> None:
    assert m.coerce_number(5) == 5.0
    assert m.coerce_number(True) == 1.0 and m.coerce_number(False) == 0.0
    assert m.coerce_number("42") == 42.0
    assert m.coerce_number("0x1c") == 28.0
    assert m.coerce_number("angrE") is None
    assert m.coerce_number(None) is None


def check_continuous_progress_never_tears_down(m) -> None:
    # permille rising every 0.5s: even out to 60s the run keeps going (progress each sample).
    wd = m.ProgressWatchdog(idle_window_seconds=1.0, hard_cap_seconds=180.0,
                            progress_keys=("oracle_loading_bar_progress_permille",))
    permille = 0
    now = 0.0
    for _ in range(120):  # 60s of 0.5s samples
        permille += 5
        d = wd.observe({"oracle_loading_bar_progress_permille": permille}, now)
        assert d == m.CONTINUE, f"unexpected {d} at t={now}"
        now += 0.5
    # A single value can exceed 1000 in this synthetic stream; the point is only-progress-continues.


def check_stall_trips_after_idle_window(m) -> None:
    # permille freezes (WORLD RES WAIT): armed run with no advance must stall out at the window.
    wd = m.ProgressWatchdog(idle_window_seconds=1.0,
                            progress_keys=("oracle_loading_bar_progress_permille",))
    assert wd.observe({"oracle_loading_bar_progress_permille": 300}, 0.0) == m.CONTINUE
    assert wd.observe({"oracle_loading_bar_progress_permille": 300}, 0.5) == m.CONTINUE
    d = wd.observe({"oracle_loading_bar_progress_permille": 300}, 1.2)
    assert d == m.TEARDOWN_STALL, d


def check_liveness_counter_does_not_mask_stall(m) -> None:
    # THE trap: present_hook_hits ticks every frame while wedged at WORLD RES WAIT. It is NOT a
    # progress key, so it must not keep the run alive -- the frozen permille still stalls.
    wd = m.ProgressWatchdog(idle_window_seconds=1.0,
                            progress_keys=("oracle_loading_bar_progress_permille",))
    hits = 0
    now = 0.0
    last = m.CONTINUE
    for _ in range(20):  # 2s of 0.1s frames: permille frozen, hits climbing fast
        hits += 40
        last = wd.observe(
            {"oracle_loading_bar_progress_permille": 300, "oracle_present_hook_hits": hits}, now)
        if last == m.TEARDOWN_STALL:
            break
        now += 0.1
    assert last == m.TEARDOWN_STALL, "liveness counter masked the stall"


def check_permille_reset_between_loads_is_not_a_stall(m) -> None:
    # First load fills to 1000, then the second load resets permille to 0 and climbs again. The
    # reset (a DECREASE) must not be scored as progress NOR as a stall; the subsequent climb is
    # progress. A naive all-time-max would think "no new max" and false-stall the whole 2nd load.
    wd = m.ProgressWatchdog(idle_window_seconds=1.0,
                            progress_keys=("oracle_loading_bar_progress_permille",))
    now = 0.0
    # first load climbs to 1000
    for permille in range(0, 1001, 100):
        assert wd.observe({"oracle_loading_bar_progress_permille": permille}, now) == m.CONTINUE
        now += 0.3
    # reset to 0 (load boundary) -- a decrease; not progress, but must not immediately stall
    assert wd.observe({"oracle_loading_bar_progress_permille": 0}, now) == m.CONTINUE
    now += 0.3
    # second load climbs from a LOW value that never exceeds the first load's 1000 peak
    for permille in range(50, 400, 50):
        d = wd.observe({"oracle_loading_bar_progress_permille": permille}, now)
        assert d == m.CONTINUE, f"second-load climb misread as {d}"
        now += 0.3


def check_terminal_semaphore_teardown_after_delay(m) -> None:
    # info arrives (terminal predicate true); tear down teardown_delay later, not immediately.
    wd = m.ProgressWatchdog(idle_window_seconds=10.0, teardown_delay_seconds=3.0,
                            terminal_predicate=lambda t: t.get("world_ready") is True,
                            progress_keys=("oracle_loading_bar_progress_permille",))
    assert wd.observe({"oracle_loading_bar_progress_permille": 100}, 0.0) == m.CONTINUE
    assert wd.observe({"world_ready": True, "oracle_loading_bar_progress_permille": 1000}, 10.0) == m.CONTINUE
    # within the delay: still running
    assert wd.observe({"world_ready": True}, 12.0) == m.CONTINUE
    # past the delay: teardown
    assert wd.observe({"world_ready": True}, 13.1) == m.TEARDOWN_TERMINAL


def check_arm_predicate_suppresses_early_stall(m) -> None:
    # Before arming (no loading screen yet), a multi-second gap with no progress must NOT stall.
    wd = m.ProgressWatchdog(idle_window_seconds=1.0,
                            arm_predicate=lambda t: m.coerce_number(
                                t.get("oracle_loading_bar_progress_permille")) not in (None, 0.0),
                            progress_keys=("oracle_loading_bar_progress_permille",))
    # 5s of pre-load with permille==0: not armed, so no stall despite no progress
    now = 0.0
    for _ in range(10):
        assert wd.observe({"oracle_loading_bar_progress_permille": 0}, now) == m.CONTINUE
        now += 0.5
    # loading screen starts (permille>0) -> arms; then it freezes -> stalls within the window
    assert wd.observe({"oracle_loading_bar_progress_permille": 50}, now) == m.CONTINUE
    now += 0.5
    assert wd.observe({"oracle_loading_bar_progress_permille": 50}, now) == m.CONTINUE
    now += 0.8
    assert wd.observe({"oracle_loading_bar_progress_permille": 50}, now) == m.TEARDOWN_STALL


def check_hard_cap_backstop(m) -> None:
    # No progress key present at all and never armed: the hard cap still bounds the run.
    wd = m.ProgressWatchdog(idle_window_seconds=1.0, hard_cap_seconds=180.0,
                            arm_predicate=lambda _t: False)  # never arms -> no stall path
    assert wd.observe({}, 0.0) == m.CONTINUE
    assert wd.observe({}, 179.0) == m.CONTINUE
    assert wd.observe({}, 180.0) == m.TEARDOWN_CAP


def main() -> int:
    m = load_mod()
    check_coerce_number(m)
    check_continuous_progress_never_tears_down(m)
    check_stall_trips_after_idle_window(m)
    check_liveness_counter_does_not_mask_stall(m)
    check_permille_reset_between_loads_is_not_a_stall(m)
    check_terminal_semaphore_teardown_after_delay(m)
    check_arm_predicate_suppresses_early_stall(m)
    check_hard_cap_backstop(m)
    print("semaphore-watchdog: all tests passed")
    return 0


if __name__ == "__main__":
    sys.exit(main())
