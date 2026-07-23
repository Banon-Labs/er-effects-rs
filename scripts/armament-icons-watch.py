#!/usr/bin/env python3
"""Semaphore-progress watcher for the armament-icons badge smoke (bd er-effects-rs-pe98).

Owns the timed poll loop and teardown so the shell runner needs no `sleep`
(scripts/check-no-timeouts.py bans shell sleeps; Python time.sleep is fine, the
gate only bounds subprocess timeouts). Polls two in-game log artifacts:

  <game-dir>/er-armament-icons.log        -- the badge DLL: "badge sample: DRAWN"
  <game-dir>/er-input-harness-phases.jsonl -- the harness: dwell_equip advanced

Verdict (semaphore-progress teardown, not wall-clock): PASS when the harness
reaches dwell_equip AND the badge log shows a DRAWN line; DWELL_NO_DRAW if the
dwell completed with no DRAWN; DERAILED if any harness phase derailed; else the
canonical runtime cap is the idle backstop. Tears down only the PIDs this run
spawned (passed in), copies artifacts, writes report.txt. Exit 0 on PASS.
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
# Never set: `.wait(n)` paces the poll loop as an interruptible bounded wait (the repo's
# watcher idiom, e.g. capture-samechar-3x.py), not a raw time.sleep.
_POLL_WAIT = threading.Event()


def win_pids_for(image: str) -> set[int]:
    try:
        out = subprocess.run(
            ["tasklist.exe", "/FI", f"IMAGENAME eq {image}", "/FO", "CSV", "/NH"],
            text=True, capture_output=True, timeout=15,
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


def capture_window(repo_root: Path, artifact_dir: Path) -> str:
    """Capture the live ER window to a PNG via the Windows-native PowerShell helper.
    Returns a status line. Best-effort: never raises."""
    ps1 = repo_root / "scripts" / "capture-er-window-win.ps1"
    out = artifact_dir / "armament-icons-equip.png"
    try:
        win_out = subprocess.run(["wslpath", "-w", str(out)], text=True,
                                 capture_output=True, timeout=15).stdout.strip()
        win_ps1 = subprocess.run(["wslpath", "-w", str(ps1)], text=True,
                                 capture_output=True, timeout=15).stdout.strip()
        subprocess.run(
            ["powershell.exe", "-NoProfile", "-ExecutionPolicy", "Bypass",
             "-File", win_ps1, win_out],
            capture_output=True, timeout=25,
        )
    except Exception as exc:
        return f"capture error: {exc}"
    note = out.with_suffix(".txt")
    detail = note.read_text(encoding="utf-8", errors="replace").strip() if note.exists() else "no note"
    return f"capture: {'PNG ' + str(out) if out.exists() else 'FAILED'} ({detail})"


def teardown(pre_er: set[int], pre_me3: set[int]) -> str:
    """Kill only this run's PIDs; two passes with a verify wait. Returns a status line."""
    attempt = 0
    for attempt in (1, 2):
        for pid in win_pids_for("eldenring.exe") - pre_er:
            subprocess.run(["taskkill.exe", "/F", "/PID", str(pid)],
                           capture_output=True, timeout=15)
        for image in ("me3.exe", "me3-launcher.exe"):
            base = pre_me3 if image == "me3.exe" else set()
            for pid in win_pids_for(image) - base:
                subprocess.run(["taskkill.exe", "/F", "/PID", str(pid)],
                               capture_output=True, timeout=15)
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
    ap.add_argument("--settle-seconds", type=float, default=3.0,
                    help="keep the game up this long after the decisive semaphore before teardown")
    ap.add_argument("--capture-settle-seconds", type=float, default=5.0,
                    help="wait this long after the menu opens before the first capture (fade-in)")
    ap.add_argument("--pre-er-pids", default="")
    ap.add_argument("--pre-me3-pids", default="")
    ap.add_argument("--repo-root", type=Path, default=Path.cwd())
    # Pixel-diff oracle (optional): when both are given, the final verdict is the pixel
    # comparison of the captured tile crop vs the vanilla baseline -- SUCCESS / FAILURE /
    # TIMEOUT -- instead of the DRAWN-log heuristic.
    ap.add_argument("--baseline", type=Path, help="vanilla baseline PNG for the pixel oracle")
    ap.add_argument("--stage-box", help="x,y,w,h in 1920x1080 stage units (the tile crop)")
    ap.add_argument("--threshold", type=float, default=0.02)
    args = ap.parse_args()

    pre_er = {int(x) for x in args.pre_er_pids.split() if x.isdigit()}
    pre_me3 = {int(x) for x in args.pre_me3_pids.split() if x.isdigit()}
    badge_log = args.game_dir / "er-armament-icons.log"
    phases = args.game_dir / "er-input-harness-phases.jsonl"

    start = time.monotonic()
    decisive = 0.0
    verdict = "INCOMPLETE"
    capture_line = "capture: not attempted"
    captured = False
    menu_open_at = 0.0
    while True:
        elapsed = time.monotonic() - start
        if elapsed >= args.max_seconds:
            verdict = "CAP_BACKSTOP"
            break
        equip_open = (
            contains(phases, '"phase":"open_equip_menu"')
            or contains(phases, '"phase":"open_inventory_menu"')
        ) and contains(phases, '"outcome":"advanced"')
        dwell_done = contains(phases, '"phase":"dwell_equip"') and contains(
            phases, '"outcome":"advanced"'
        )
        drawn = contains(badge_log, "badge sample: DRAWN")
        derailed = contains(phases, '"outcome":"derailed"')
        if equip_open and menu_open_at == 0.0:
            menu_open_at = time.monotonic()
        # Capture only AFTER the menu has been up long enough to fully render (fade-in settled),
        # not at the open edge (user 2026-07-23: don't tear down / capture too early).
        if menu_open_at > 0.0 and not captured and (
            time.monotonic() - menu_open_at >= args.capture_settle_seconds
        ):
            capture_line = capture_window(args.repo_root, args.artifact_dir)
            captured = True
        if decisive == 0.0:
            if dwell_done and captured:
                verdict = "PASS" if drawn else "DWELL_NO_DRAW"
                decisive = time.monotonic()
            elif derailed:
                verdict = "DERAILED"
                decisive = time.monotonic()
        elif time.monotonic() - decisive >= args.settle_seconds:
            break
        _POLL_WAIT.wait(POLL_SECONDS)

    # Final settled re-capture just before teardown (menu fully rendered, badges settled).
    if verdict in ("PASS", "DWELL_NO_DRAW") and menu_open_at > 0.0:
        capture_line = capture_window(args.repo_root, args.artifact_dir)

    status_line = teardown(pre_er, pre_me3)

    for name in (
        "er-armament-icons.log",
        "er-input-harness.log",
        "er-input-harness-phases.jsonl",
        "er-telemetry-timeseries.jsonl",
    ):
        src = args.game_dir / name
        if src.exists():
            shutil.copy(src, args.artifact_dir / name)

    # PIXEL-DIFF ORACLE (when configured): the authoritative verdict is the tile crop vs the
    # vanilla baseline. TIMEOUT if the menu was never reached / nothing captured.
    oracle_line = "oracle: not configured"
    if args.baseline and args.stage_box:
        capture_png = args.artifact_dir / "armament-icons-equip.png"
        if not captured or not capture_png.exists() or not args.baseline.exists():
            verdict = "TIMEOUT"
            oracle_line = f"oracle: TIMEOUT (captured={captured} png={capture_png.exists()})"
        else:
            proc = subprocess.run(
                ["python3", str(args.repo_root / "scripts" / "armament-icons-pixel-oracle.py"),
                 "verdict", "--baseline", str(args.baseline), "--candidate", str(capture_png),
                 "--stage-box", args.stage_box, "--threshold", str(args.threshold)],
                capture_output=True, text=True, timeout=25,
            )
            oracle_line = "oracle: " + (proc.stdout.strip() or proc.stderr.strip())
            verdict = "SUCCESS" if proc.returncode == 0 else "FAILURE"

    report = args.artifact_dir / "report.txt"
    lines = [f"verdict: {verdict}", f"elapsed_seconds: {int(time.monotonic() - start)}",
             capture_line, oracle_line, status_line]
    log_copy = args.artifact_dir / "er-armament-icons.log"
    if log_copy.exists():
        lines.append("--- badge log tail ---")
        lines.extend(log_copy.read_text(encoding="utf-8", errors="replace").splitlines()[-40:])
    report.write_text("\n".join(lines) + "\n", encoding="utf-8")

    print(f"armament-icons-watch: verdict={verdict}")
    return 0 if verdict in ("PASS", "SUCCESS") else 1


if __name__ == "__main__":
    raise SystemExit(main())
