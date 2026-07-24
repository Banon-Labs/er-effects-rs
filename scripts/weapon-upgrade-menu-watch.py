#!/usr/bin/env python3
"""Semaphore watcher for the weapon-upgrade menu probe.

The shell runner stages the input harness + telemetry DLLs and sets
`er-harness-drive-mode.txt` to `upgrade` or `upgrade_det`. This watcher owns the bounded poll loop
and teardown. It never uses screenshots; verdict comes from harness phase telemetry:

- PASS (`upgrade`): `open_weapon_upgrade_menu` advanced and the following dwell advanced.
- PASS (`upgrade_det`): deterministic seed, open menu, strengthen-dialog build, buffered OK, OK-effect transition, and dwell all advanced.
- DERAILED: any harness phase derailed.
- NO_HARNESS_PHASES: no harness log/phase telemetry appeared promptly after launch.
- NO_PHASE_PROGRESS: harness telemetry appeared but stopped advancing before a decisive verdict.
- CAP_BACKSTOP: no decisive phase evidence before the runtime cap.

Only PIDs spawned by this run are torn down.
"""

from __future__ import annotations

import argparse
import contextlib
import csv
import json
import shutil
import subprocess
import threading
import time
from pathlib import Path

POLL_SECONDS = 2.0
KILL_VERIFY_SECONDS = 2.0
TASKLIST_TIMEOUT_SECONDS = 3
TASKKILL_TIMEOUT_SECONDS = 5
NO_HARNESS_PHASES_SECONDS = 12.0
NO_PHASE_PROGRESS_SECONDS = 12.0
_POLL_WAIT = threading.Event()


def win_pids_for(image: str) -> set[int]:
    try:
        out = subprocess.run(
            ["tasklist.exe", "/FI", f"IMAGENAME eq {image}", "/FO", "CSV", "/NH"],
            text=True,
            capture_output=True,
            timeout=TASKLIST_TIMEOUT_SECONDS,
        ).stdout
    except Exception:
        return set()
    pids: set[int] = set()
    for row in csv.reader(out.splitlines()):
        if len(row) > 1 and row[1].isdigit():
            pids.add(int(row[1]))
    return pids


def phase_events(path: Path) -> list[dict[str, object]]:
    try:
        raw_lines = path.read_text(encoding="utf-8", errors="replace").splitlines()
    except OSError:
        return []
    events: list[dict[str, object]] = []
    for line in raw_lines:
        try:
            event = json.loads(line)
        except json.JSONDecodeError:
            continue
        if isinstance(event, dict):
            events.append(event)
    return events


def phase_outcomes(events: list[dict[str, object]]) -> dict[str, set[str]]:
    outcomes: dict[str, set[str]] = {}
    for event in events:
        phase = event.get("phase")
        outcome = event.get("outcome")
        if isinstance(phase, str) and isinstance(outcome, str):
            outcomes.setdefault(phase, set()).add(outcome)
    return outcomes


def newest_mtime(paths: tuple[Path, ...]) -> float:
    latest = 0.0
    for path in paths:
        with contextlib.suppress(OSError):
            latest = max(latest, path.stat().st_mtime)
    return latest


def last_phase_fields(events: list[dict[str, object]]) -> tuple[str, str, str]:
    if not events:
        return "", "", ""
    last = events[-1]
    return (
        str(last.get("phase", "")),
        str(last.get("outcome", "")),
        str(last.get("duration_frames", last.get("end_frame", ""))),
    )


def teardown(pre_er: set[int], pre_me3: set[int]) -> str:
    attempt = 0
    for _ in (1, 2):
        attempt += 1
        for pid in win_pids_for("eldenring.exe") - pre_er:
            subprocess.run(
                ["taskkill.exe", "/F", "/PID", str(pid)],
                capture_output=True,
                timeout=TASKKILL_TIMEOUT_SECONDS,
            )
        for image in ("me3.exe", "me3-launcher.exe"):
            for pid in win_pids_for(image) - pre_me3:
                subprocess.run(
                    ["taskkill.exe", "/F", "/PID", str(pid)],
                    capture_output=True,
                    timeout=TASKKILL_TIMEOUT_SECONDS,
                )
        _POLL_WAIT.wait(KILL_VERIFY_SECONDS)
        if not (win_pids_for("eldenring.exe") - pre_er):
            break
    remaining = win_pids_for("eldenring.exe") - pre_er
    return f"post-cleanup attempt={attempt} eldenring_remaining={sorted(remaining)}"


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--game-dir", required=True, type=Path)
    ap.add_argument("--artifact-dir", required=True, type=Path)
    ap.add_argument("--max-seconds", required=True, type=float)
    ap.add_argument("--settle-seconds", type=float, default=3.0)
    ap.add_argument("--pre-er-pids", default="")
    ap.add_argument("--pre-me3-pids", default="")
    ap.add_argument("--mode", choices=("upgrade", "upgrade_det"), default="upgrade")
    ap.add_argument(
        "--no-harness-seconds", type=float, default=NO_HARNESS_PHASES_SECONDS
    )
    ap.add_argument(
        "--no-progress-seconds", type=float, default=NO_PHASE_PROGRESS_SECONDS
    )
    args = ap.parse_args()

    pre_er = {int(x) for x in args.pre_er_pids.split() if x.isdigit()}
    pre_me3 = {int(x) for x in args.pre_me3_pids.split() if x.isdigit()}
    phases = args.game_dir / "er-input-harness-phases.jsonl"
    harness_log = args.game_dir / "er-input-harness.log"

    start = time.monotonic()
    decisive = 0.0
    verdict = "INCOMPLETE"
    last_activity_mtime = 0.0
    last_activity_seen = start
    last_events: list[dict[str, object]] = []
    while True:
        now = time.monotonic()
        elapsed = now - start
        events = phase_events(phases)
        last_events = events
        outcomes = phase_outcomes(events)
        activity_mtime = newest_mtime((phases, harness_log))
        if activity_mtime > last_activity_mtime:
            last_activity_mtime = activity_mtime
            last_activity_seen = now
        if (
            not outcomes
            and not harness_log.exists()
            and elapsed >= args.no_harness_seconds
        ):
            verdict = "NO_HARNESS_PHASES"
            break
        if (
            outcomes or harness_log.exists()
        ) and now - last_activity_seen >= args.no_progress_seconds:
            verdict = "NO_PHASE_PROGRESS"
            break
        if elapsed >= args.max_seconds:
            verdict = "CAP_BACKSTOP"
            break
        opened = "advanced" in outcomes.get("open_weapon_upgrade_menu", set())
        dwell_done = "advanced" in outcomes.get("dwell_equip", set())
        if args.mode == "upgrade_det":
            seed_done = "advanced" in outcomes.get(
                "grant_deterministic_strengthen_seed", set()
            )
            dialog_done = "advanced" in outcomes.get("build_strengthen_dialog", set())
            buffered_ok_done = "advanced" in outcomes.get(
                "buffered_strengthen_dialog_ok", set()
            )
            ok_effect_done = "advanced" in outcomes.get(
                "wait_buffered_dialog_ok_effect", set()
            )
            pass_ready = (
                seed_done
                and opened
                and dialog_done
                and buffered_ok_done
                and ok_effect_done
                and dwell_done
            )
        else:
            pass_ready = opened and dwell_done
        derailed = any(
            "derailed" in outcomes_for_phase for outcomes_for_phase in outcomes.values()
        )
        if decisive == 0.0:
            if pass_ready:
                verdict = "PASS"
                decisive = time.monotonic()
            elif derailed:
                verdict = "DERAILED"
                decisive = time.monotonic()
        elif time.monotonic() - decisive >= args.settle_seconds:
            break
        _POLL_WAIT.wait(POLL_SECONDS)

    status_line = teardown(pre_er, pre_me3)
    last_phase, last_outcome, last_phase_frame = last_phase_fields(last_events)

    for name in (
        "er-input-harness.log",
        "er-input-harness-phases.jsonl",
        "er-telemetry-timeseries.jsonl",
    ):
        src = args.game_dir / name
        if src.exists():
            shutil.copy(src, args.artifact_dir / name)

    report = args.artifact_dir / "report.txt"
    lines = [
        f"verdict: {verdict}",
        f"elapsed_seconds: {int(time.monotonic() - start)}",
        f"harness_log_exists: {harness_log.exists()}",
        f"phases_exists: {phases.exists()}",
        f"mode: {args.mode}",
        status_line,
        f"last_activity_idle_seconds: {int(time.monotonic() - last_activity_seen)}",
        f"last_phase: {last_phase}",
        f"last_phase_outcome: {last_outcome}",
        f"last_phase_frame: {last_phase_frame}",
    ]
    phase_copy = args.artifact_dir / "er-input-harness-phases.jsonl"
    if phase_copy.exists():
        lines.append("--- harness phases ---")
        lines.extend(
            phase_copy.read_text(encoding="utf-8", errors="replace").splitlines()
        )
    report.write_text("\n".join(lines) + "\n", encoding="utf-8")

    print(f"weapon-upgrade-menu-watch: verdict={verdict}")
    return 0 if verdict == "PASS" else 1


if __name__ == "__main__":
    raise SystemExit(main())
