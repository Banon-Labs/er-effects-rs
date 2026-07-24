#!/usr/bin/env python3
"""Regression tests for scripts/weapon-upgrade-menu-watch.py.

These tests are no-launch/no-Windows-process tests. They import the watcher, monkeypatch process
lookup to an empty set, and drive synthetic harness files so the teardown verdicts that protect the
user's desktop remain executable.
"""
from __future__ import annotations

import importlib.util
import json
import sys
import tempfile
from pathlib import Path
from typing import Any, cast

REPO_ROOT = Path(__file__).resolve().parents[1]
WATCH_PATH = REPO_ROOT / "scripts" / "weapon-upgrade-menu-watch.py"


def load_watcher():
    spec = importlib.util.spec_from_file_location("weapon_upgrade_menu_watch", WATCH_PATH)
    if spec is None or spec.loader is None:
        raise RuntimeError(f"failed to load {WATCH_PATH}")
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    watcher = cast(Any, module)
    watcher.win_pids_for = lambda _image: set()
    watcher.POLL_SECONDS = 0.05
    watcher.KILL_VERIFY_SECONDS = 0.0
    return module


def run_watch(watcher, game_dir: Path, artifact_dir: Path, *extra: str) -> tuple[int, str]:
    old_argv = sys.argv[:]
    sys.argv = [
        str(WATCH_PATH),
        "--game-dir",
        str(game_dir),
        "--artifact-dir",
        str(artifact_dir),
        "--max-seconds",
        "2",
        "--settle-seconds",
        "0",
        "--mode",
        "upgrade_det",
        *extra,
    ]
    try:
        rc = watcher.main()
    finally:
        sys.argv = old_argv
    report = (artifact_dir / "report.txt").read_text(encoding="utf-8", errors="replace")
    return rc, report


def test_no_harness_phases(watcher) -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        game_dir = root / "game"
        artifact_dir = root / "artifact"
        game_dir.mkdir()
        artifact_dir.mkdir()
        rc, report = run_watch(
            watcher, game_dir, artifact_dir, "--no-harness-seconds", "0.1"
        )
    assert rc == 1
    assert "verdict: NO_HARNESS_PHASES" in report
    assert "post-cleanup" in report


def test_no_phase_progress_reports_last_phase(watcher) -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        game_dir = root / "game"
        artifact_dir = root / "artifact"
        game_dir.mkdir()
        artifact_dir.mkdir()
        (game_dir / "er-input-harness.log").write_text(
            "phase[3] wait_load_in ENTER\n", encoding="utf-8"
        )
        (game_dir / "er-input-harness-phases.jsonl").write_text(
            json.dumps(
                {
                    "phase": "wait_load_in",
                    "outcome": "heartbeat",
                    "duration_frames": 60,
                    "strengthen_row_kind": "none",
                }
            )
            + "\n",
            encoding="utf-8",
        )
        rc, report = run_watch(
            watcher, game_dir, artifact_dir, "--no-progress-seconds", "0.1"
        )
    assert rc == 1
    assert "verdict: NO_PHASE_PROGRESS" in report
    assert "last_phase: wait_load_in" in report
    assert "last_phase_outcome: heartbeat" in report
    assert "last_phase_frame: 60" in report


def test_upgrade_det_pass_requires_all_expected_phases(watcher) -> None:
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        game_dir = root / "game"
        artifact_dir = root / "artifact"
        game_dir.mkdir()
        artifact_dir.mkdir()
        phases = [
            "grant_deterministic_strengthen_seed",
            "open_weapon_upgrade_menu",
            "build_strengthen_dialog",
            "buffered_strengthen_dialog_ok",
            "wait_buffered_dialog_ok_effect",
            "dwell_equip",
        ]
        (game_dir / "er-input-harness.log").write_text("ok\n", encoding="utf-8")
        (game_dir / "er-input-harness-phases.jsonl").write_text(
            "\n".join(
                json.dumps(
                    {
                        "phase": phase,
                        "outcome": "advanced",
                        "duration_frames": index,
                        "strengthen_row_kind": "weapon",
                    }
                )
                for index, phase in enumerate(phases, start=1)
            )
            + "\n",
            encoding="utf-8",
        )
        rc, report = run_watch(watcher, game_dir, artifact_dir)
    assert rc == 0
    assert "verdict: PASS" in report
    assert "last_phase: dwell_equip" in report
    assert "last_phase_outcome: advanced" in report


def main() -> int:
    watcher = load_watcher()
    test_no_harness_phases(watcher)
    test_no_phase_progress_reports_last_phase(watcher)
    test_upgrade_det_pass_requires_all_expected_phases(watcher)
    print("weapon-upgrade-menu-watch: all tests passed")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
