#!/usr/bin/env python3
"""Semaphore watcher for the weapon-upgrade menu probe.

The shell runner stages the input harness + telemetry DLLs and sets
`er-harness-drive-mode.txt=upgrade`. This watcher owns the bounded poll loop and
teardown. It never uses screenshots; verdict comes from harness phase telemetry:

- PASS: `open_weapon_upgrade_menu` advanced and the following dwell advanced.
- DERAILED: any harness phase derailed.
- CAP_BACKSTOP: no decisive phase evidence before the runtime cap.

Only PIDs spawned by this run are torn down.
"""

from __future__ import annotations

import argparse
import csv
import shutil
import subprocess
import threading
import time
from pathlib import Path

POLL_SECONDS = 2.0
KILL_VERIFY_SECONDS = 2.0
_POLL_WAIT = threading.Event()


def win_pids_for(image: str) -> set[int]:
    try:
        out = subprocess.run(
            ["tasklist.exe", "/FI", f"IMAGENAME eq {image}", "/FO", "CSV", "/NH"],
            text=True,
            capture_output=True,
            timeout=15,
        ).stdout
    except Exception:
        return set()
    pids: set[int] = set()
    for row in csv.reader(out.splitlines()):
        if len(row) > 1 and row[1].isdigit():
            pids.add(int(row[1]))
    return pids


def contains(path: Path, needle: str) -> bool:
    try:
        return needle in path.read_text(encoding="utf-8", errors="replace")
    except OSError:
        return False


def teardown(pre_er: set[int], pre_me3: set[int]) -> str:
    attempt = 0
    for _ in (1, 2):
        attempt += 1
        for pid in win_pids_for("eldenring.exe") - pre_er:
            subprocess.run(
                ["taskkill.exe", "/F", "/PID", str(pid)],
                capture_output=True,
                timeout=15,
            )
        for image in ("me3.exe", "me3-launcher.exe"):
            baseline = pre_me3 if image == "me3.exe" else set()
            for pid in win_pids_for(image) - baseline:
                subprocess.run(
                    ["taskkill.exe", "/F", "/PID", str(pid)],
                    capture_output=True,
                    timeout=15,
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
    args = ap.parse_args()

    pre_er = {int(x) for x in args.pre_er_pids.split() if x.isdigit()}
    pre_me3 = {int(x) for x in args.pre_me3_pids.split() if x.isdigit()}
    phases = args.game_dir / "er-input-harness-phases.jsonl"

    start = time.monotonic()
    decisive = 0.0
    verdict = "INCOMPLETE"
    while True:
        elapsed = time.monotonic() - start
        if elapsed >= args.max_seconds:
            verdict = "CAP_BACKSTOP"
            break
        opened = contains(phases, '"phase":"open_weapon_upgrade_menu"') and contains(
            phases, '"outcome":"advanced"'
        )
        dwell_done = contains(phases, '"phase":"dwell_equip"') and contains(
            phases, '"outcome":"advanced"'
        )
        derailed = contains(phases, '"outcome":"derailed"')
        if decisive == 0.0:
            if opened and dwell_done:
                verdict = "PASS"
                decisive = time.monotonic()
            elif derailed:
                verdict = "DERAILED"
                decisive = time.monotonic()
        elif time.monotonic() - decisive >= args.settle_seconds:
            break
        _POLL_WAIT.wait(POLL_SECONDS)

    status_line = teardown(pre_er, pre_me3)

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
        status_line,
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
