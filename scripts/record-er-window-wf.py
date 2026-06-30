#!/usr/bin/env python3
"""Record the validated Elden Ring Hyprland window with wf-recorder.

Target-window-only recording helper for visual proof runs:
- waits for class=steam_app_1245620 to exist, be mapped, not hidden, and focused/topmost;
- never focuses, raises, floats, moves, or resizes the target;
- waits until the same address has stable natural geometry for several samples;
- records the exact validated live geometry;
- fails before starting wf-recorder if no stable target exists.

This deliberately does not trust preselected window sizes or hypr-window-placer
records. If the window never stabilizes, there is no recording request/result and
callers must not claim a visual run started.
"""
from __future__ import annotations

import argparse
import json
import shutil
import subprocess
import sys
import time
from pathlib import Path
from typing import Any

WINDOW_CLASS = "steam_app_1245620"


def run(args: list[str], timeout: float | None = 10) -> subprocess.CompletedProcess[str]:
    return subprocess.run(args, text=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=timeout)


def hyprctl_path() -> str | None:
    return shutil.which("hyprctl")


def find_windows() -> list[dict[str, Any]]:
    hyprctl = hyprctl_path()
    if not hyprctl:
        return []
    try:
        clients = json.loads(run([hyprctl, "-j", "clients"], timeout=10).stdout)
    except Exception:
        return []
    return [c for c in clients if isinstance(c, dict) and c.get("class") == WINDOW_CLASS]


def sane_window(w: dict[str, Any] | None) -> bool:
    if not w or w.get("mapped") is False or w.get("hidden") is True:
        return False
    at = w.get("at") or []
    size = w.get("size") or []
    if len(at) != 2 or len(size) != 2:
        return False
    try:
        x, y = int(at[0]), int(at[1])
        sx, sy = int(size[0]), int(size[1])
    except Exception:
        return False
    return x >= 0 and y >= 0 and sx >= 320 and sy >= 240


def summarize(w: dict[str, Any] | None) -> dict[str, Any] | None:
    if not w:
        return None
    return {k: w.get(k) for k in ("address", "class", "at", "size", "mapped", "hidden", "focusHistoryID", "fullscreen", "workspace", "monitor", "floating", "pid")}


def best_target() -> dict[str, Any] | None:
    windows = [w for w in find_windows() if sane_window(w)]
    if not windows:
        return None
    return sorted(windows, key=lambda w: w.get("focusHistoryID", 999999))[0]


def refetch(address: str) -> dict[str, Any] | None:
    for w in find_windows():
        if w.get("address") == address:
            return w
    return None


def wait_for_stable_window(timeout_s: float, samples: int, interval_s: float, require_focus: bool) -> tuple[dict[str, Any], dict[str, Any]]:
    deadline = time.time() + timeout_s
    last_key: tuple[Any, ...] | None = None
    stable_count = 0
    last_reason = "not started"
    attempts: list[dict[str, Any]] = []

    while time.time() < deadline:
        target = best_target()
        if not target:
            last_reason = "no sane mapped target window"
            attempts.append({"event": "no_sane_target"})
            time.sleep(interval_s)
            continue

        address = str(target.get("address"))
        target = refetch(address) or target

        if not sane_window(target):
            last_reason = f"target became unsafe: {summarize(target)}"
            attempts.append({"event": "unsafe_target", "window": summarize(target)})
            stable_count = 0
            last_key = None
            time.sleep(interval_s)
            continue

        if require_focus and target.get("focusHistoryID") != 0:
            last_reason = f"target not focused/topmost: focusHistoryID={target.get('focusHistoryID')}"
            attempts.append({"event": "not_focused", "window": summarize(target)})
            stable_count = 0
            last_key = None
            time.sleep(interval_s)
            continue

        key = (target.get("address"), tuple(map(int, target.get("at") or [])), tuple(map(int, target.get("size") or [])))
        if key == last_key:
            stable_count += 1
        else:
            last_key = key
            stable_count = 1
        attempts.append({"event": "sample", "stable_count": stable_count, "window": summarize(target)})
        if stable_count >= samples:
            return target, {"attempts_tail": attempts[-32:], "stable_samples": stable_count}
        time.sleep(interval_s)

    raise SystemExit(f"no stable focused ER window before recording: {last_reason}")


def telemetry_true(path: Path, key: str) -> bool:
    try:
        value = json.loads(path.read_text()).get(key)
    except Exception:
        return False
    return bool(value)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("artifact_dir", type=Path)
    parser.add_argument("seconds", type=float, help="maximum recording duration after the stable ER window is found")
    parser.add_argument("fps", type=float)
    parser.add_argument("--window-timeout", type=float, default=45.0)
    parser.add_argument("--stable-samples", type=int, default=8)
    parser.add_argument("--stable-interval", type=float, default=0.15)
    parser.add_argument("--allow-unfocused", action="store_true", help="debug only: record even if Hyprland does not report focusHistoryID==0")
    parser.add_argument("--stop-after-player-present", action="store_true", help="keep recording until oracle_player_present, then hold before stopping")
    parser.add_argument("--telemetry", type=Path, help="telemetry JSON path; defaults to <artifact_dir>/er-effects-telemetry.json")
    parser.add_argument("--min-seconds", type=float, default=0.0, help="minimum recording duration even if the stop condition appears early")
    parser.add_argument("--post-confirm-seconds", type=float, default=8.0, help="extra recording time after player-present is first observed")
    args = parser.parse_args()

    wf = shutil.which("wf-recorder")
    if not wf:
        raise SystemExit("missing wf-recorder")
    args.artifact_dir.mkdir(parents=True, exist_ok=True)

    w, proof = wait_for_stable_window(
        timeout_s=args.window_timeout,
        samples=max(args.stable_samples, 2),
        interval_s=max(args.stable_interval, 0.05),
        require_focus=not args.allow_unfocused,
    )
    at = list(map(int, w["at"]))
    size = list(map(int, w["size"]))
    geom = f"{at[0]},{at[1]} {size[0]}x{size[1]}"
    out = args.artifact_dir / f"wf-{args.fps:g}fps.mkv"
    telemetry_path = args.telemetry or (args.artifact_dir / "er-effects-telemetry.json")
    meta = {
        "window": summarize(w),
        "stability_proof": proof,
        "geometry": geom,
        "max_seconds": args.seconds,
        "fps": args.fps,
        "output": str(out),
        "started_recording": True,
        "stop_after_player_present": args.stop_after_player_present,
        "telemetry": str(telemetry_path),
        "min_seconds": args.min_seconds,
        "post_confirm_seconds": args.post_confirm_seconds,
    }
    (args.artifact_dir / "wf-recorder-request.json").write_text(json.dumps(meta, indent=2))
    cmd = [wf, "-g", geom, "-r", str(args.fps), "-D", "-f", str(out)]
    proc = subprocess.Popen(cmd, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
    start = time.time()
    stop_reason = "max_seconds"
    player_present_at: float | None = None
    while True:
        elapsed = time.time() - start
        if elapsed >= args.seconds:
            break
        if args.stop_after_player_present and elapsed >= args.min_seconds:
            if player_present_at is None and telemetry_true(telemetry_path, "oracle_player_present"):
                player_present_at = time.time()
                stop_reason = "player_present_hold"
            if player_present_at is not None and time.time() - player_present_at >= args.post_confirm_seconds:
                break
        time.sleep(0.2)
    proc.terminate()
    try:
        stdout, stderr = proc.communicate(timeout=5)
    except subprocess.TimeoutExpired:
        proc.kill()
        stdout, stderr = proc.communicate()
    result = {
        "cmd": cmd,
        "returncode": proc.returncode,
        "stdout": stdout,
        "stderr": stderr,
        "output_exists": out.exists(),
        "output_size": out.stat().st_size if out.exists() else 0,
        "geometry": geom,
        "window": summarize(w),
        "recorded_seconds": round(time.time() - start, 3),
        "stop_reason": stop_reason,
        "player_present_observed": player_present_at is not None,
    }
    (args.artifact_dir / "wf-recorder-result.json").write_text(json.dumps(result, indent=2))
    print(json.dumps({"done": True, **result}))
    return 0 if out.exists() and out.stat().st_size > 0 else 1


if __name__ == "__main__":
    raise SystemExit(main())
