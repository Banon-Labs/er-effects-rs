#!/usr/bin/env python3
"""Regression tests for scripts/er-readiness-watch.py."""
from __future__ import annotations

import importlib.util
import sys
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[1]
WATCH_PATH = REPO_ROOT / "scripts" / "er-readiness-watch.py"
TEST_PID = 4242
TEST_POLLS = 7
TEST_BUDGET = 3
TEST_WINDOW = {"class": "steam_app_1245620", "title": "ELDEN RING™"}


def load_watcher():
    spec = importlib.util.spec_from_file_location("er_readiness_watch", WATCH_PATH)
    if spec is None or spec.loader is None:
        raise RuntimeError(f"failed to load {WATCH_PATH}")
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def main() -> int:
    watcher = load_watcher()

    assert watcher.client_is_game_window(TEST_WINDOW, watcher.DEFAULT_WINDOW_CLASS)
    assert not watcher.client_is_game_window(
        {
            "class": "steam_proton",
            "title": "Z:\\home\\banon\\.local\\share\\Steam\\steamapps\\common\\ELDEN RING\\Game\\start_protected_game.exe",
        },
        watcher.DEFAULT_WINDOW_CLASS,
    )

    ready = watcher.classify_snapshot(
        pid=TEST_PID,
        process_running=True,
        telemetry={"game_man_available": True},
        bootstrap=None,
        windows=[],
        window_stale_polls=0,
        window_stale_poll_budget=TEST_BUDGET,
        polls=TEST_POLLS,
    )
    assert ready is not None
    assert ready.ready
    assert ready.reason == watcher.READY_REASON

    exited = watcher.classify_snapshot(
        pid=TEST_PID,
        process_running=False,
        telemetry=None,
        bootstrap=None,
        windows=[],
        window_stale_polls=0,
        window_stale_poll_budget=TEST_BUDGET,
        polls=TEST_POLLS,
    )
    assert exited is not None
    assert not exited.ready
    assert exited.reason == watcher.PROCESS_EXITED

    no_bootstrap = watcher.classify_snapshot(
        pid=TEST_PID,
        process_running=True,
        telemetry=None,
        bootstrap=None,
        windows=[TEST_WINDOW],
        window_stale_polls=1,
        window_stale_poll_budget=TEST_BUDGET,
        polls=TEST_POLLS,
    )
    assert no_bootstrap is not None
    assert not no_bootstrap.ready
    assert no_bootstrap.reason == watcher.WINDOW_WITHOUT_BOOTSTRAP

    waiting_for_task = watcher.classify_snapshot(
        pid=TEST_PID,
        process_running=True,
        telemetry=None,
        bootstrap={"stage": "game_task_thread_started"},
        windows=[TEST_WINDOW],
        window_stale_polls=TEST_BUDGET - 1,
        window_stale_poll_budget=TEST_BUDGET,
        polls=TEST_POLLS,
    )
    assert waiting_for_task is None

    no_task = watcher.classify_snapshot(
        pid=TEST_PID,
        process_running=True,
        telemetry=None,
        bootstrap={"stage": "game_task_thread_started"},
        windows=[TEST_WINDOW],
        window_stale_polls=TEST_BUDGET,
        window_stale_poll_budget=TEST_BUDGET,
        polls=TEST_POLLS,
    )
    assert no_task is not None
    assert not no_task.ready
    assert no_task.reason == watcher.WINDOW_WITHOUT_TASK

    telemetry_without_game_man = watcher.classify_snapshot(
        pid=TEST_PID,
        process_running=True,
        telemetry={"game_man_available": False},
        bootstrap={"stage": "telemetry_write"},
        windows=[TEST_WINDOW],
        window_stale_polls=TEST_BUDGET,
        window_stale_poll_budget=TEST_BUDGET,
        polls=TEST_POLLS,
    )
    assert telemetry_without_game_man is not None
    assert not telemetry_without_game_man.ready
    assert telemetry_without_game_man.reason == watcher.TELEMETRY_WITHOUT_GAME_MAN

    no_telemetry = watcher.classify_snapshot(
        pid=TEST_PID,
        process_running=True,
        telemetry=None,
        bootstrap={"stage": "telemetry_write"},
        windows=[TEST_WINDOW],
        window_stale_polls=TEST_BUDGET,
        window_stale_poll_budget=TEST_BUDGET,
        polls=TEST_POLLS,
    )
    assert no_telemetry is not None
    assert not no_telemetry.ready
    assert no_telemetry.reason == watcher.WINDOW_WITHOUT_TELEMETRY

    print("er-readiness-watch regression tests passed")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
