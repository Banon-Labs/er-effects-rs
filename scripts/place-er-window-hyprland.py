#!/usr/bin/env python3
"""Keep the Elden Ring Hyprland window inside a real monitor.

This is intentionally target-only: it filters for class=steam_app_1245620 and
uses the resolved window address for every dispatcher. It never falls back to the
active window.
"""

from __future__ import annotations

import argparse
import json
import subprocess
import sys
import time
from pathlib import Path
from typing import Any

TARGET_CLASS = "steam_app_1245620"


def hypr_json(*args: str) -> Any:
    out = subprocess.check_output(["hyprctl", "-j", *args], text=True, stderr=subprocess.STDOUT, timeout=5)
    return json.loads(out)


def hypr_dispatch(expr: str) -> tuple[int, str, str]:
    cp = subprocess.run(["hyprctl", "dispatch", expr], text=True, capture_output=True, timeout=5)
    return cp.returncode, cp.stdout.strip(), cp.stderr.strip()


def target_windows(window_class: str) -> list[dict[str, Any]]:
    return [c for c in hypr_json("clients") if c.get("class") == window_class and c.get("mapped") and not c.get("hidden")]


def monitor_for(monitors: list[dict[str, Any]], spec: str, window: dict[str, Any] | None) -> dict[str, Any] | None:
    if spec == "window" and window is not None:
        win_monitor = window.get("monitor")
        for mon in monitors:
            if mon.get("id") == win_monitor:
                return mon
    if spec == "focused":
        for mon in monitors:
            if mon.get("focused"):
                return mon
    for mon in monitors:
        if str(mon.get("id")) == spec or mon.get("name") == spec:
            return mon
    return None


def workspace_for(spec: str, monitor: dict[str, Any], window: dict[str, Any]) -> int | None:
    if spec == "none":
        return None
    if spec == "window":
        ws = window.get("workspace") or {}
        return ws.get("id")
    if spec == "monitor":
        ws = monitor.get("activeWorkspace") or {}
        return ws.get("id")
    return int(spec)


def window_summary(window: dict[str, Any] | None) -> dict[str, Any] | None:
    if window is None:
        return None
    return {k: window.get(k) for k in ("address", "at", "size", "workspace", "monitor", "floating", "focusHistoryID", "pid")}


def intersects_monitor(window: dict[str, Any], monitor: dict[str, Any]) -> bool:
    wx, wy = window.get("at") or [0, 0]
    ww, wh = window.get("size") or [0, 0]
    mx, my, mw, mh = monitor["x"], monitor["y"], monitor["width"], monitor["height"]
    return wx < mx + mw and wx + ww > mx and wy < my + mh and wy + wh > my


def emit(log: Path | None, event: dict[str, Any]) -> None:
    line = json.dumps(event, sort_keys=True)
    print(line, flush=True)
    if log is not None:
        with log.open("a", encoding="utf-8") as f:
            f.write(line + "\n")


def place_once(args: argparse.Namespace, log: Path | None) -> bool:
    windows = target_windows(args.window_class)
    if not windows:
        emit(log, {"event": "no_target_window", "class": args.window_class})
        return False

    # Prefer the already-focused target when multiple transient ER/XWayland windows exist;
    # otherwise pick the first matching mapped window. This is still target-only.
    window = sorted(windows, key=lambda w: w.get("focusHistoryID", 999999))[0]
    before = window_summary(window)
    monitors = hypr_json("monitors")
    monitor = monitor_for(monitors, args.monitor, window)
    if monitor is None:
        emit(log, {"event": "no_target_monitor", "monitor_spec": args.monitor, "before": before})
        return False

    target_w = min(args.width, monitor["width"])
    target_h = min(args.height, monitor["height"])
    target_x = monitor["x"] + max((monitor["width"] - target_w) // 2, 0)
    target_y = monitor["y"] + max((monitor["height"] - target_h) // 2, 0)
    selector = f"address:{window['address']}"

    should_place = args.always or not intersects_monitor(window, monitor) or window.get("size") != [target_w, target_h]
    if not should_place:
        emit(log, {"event": "already_visible", "monitor": monitor.get("name"), "before": before})
        return True

    commands: list[str] = []
    workspace = workspace_for(args.workspace, monitor, window)
    if workspace is not None:
        commands.append(f'hl.dsp.window.move({{ workspace = {workspace}, window = "{selector}" }})')
    if args.monitor != "window":
        # When an explicit monitor is requested, make that binding real before absolute coordinates.
        mon_value = monitor.get("id") if str(monitor.get("id")) == args.monitor else monitor.get("name")
        if isinstance(mon_value, str):
            commands.append(f'hl.dsp.window.move({{ monitor = "{mon_value}", window = "{selector}" }})')
        else:
            commands.append(f'hl.dsp.window.move({{ monitor = {mon_value}, window = "{selector}" }})')
    commands.extend([
        f'hl.dsp.window.float({{ action = "enable", window = "{selector}" }})',
        f'hl.dsp.window.resize({{ x = {target_w}, y = {target_h}, window = "{selector}" }})',
        f'hl.dsp.window.move({{ x = {target_x}, y = {target_y}, window = "{selector}" }})',
    ])
    if args.focus:
        commands.append(f'hl.dsp.focus({{ window = "{selector}" }})')

    results = []
    for expr in commands:
        rc, stdout, stderr = hypr_dispatch(expr)
        results.append({"expr": expr, "rc": rc, "stdout": stdout, "stderr": stderr})
        if rc != 0 or stdout.startswith("error:"):
            emit(log, {"event": "dispatch_failed", "before": before, "result": results[-1]})
            return False

    after = None
    for candidate in target_windows(args.window_class):
        if candidate.get("address") == window.get("address"):
            after = candidate
            break
    ok = after is not None and intersects_monitor(after, monitor)
    emit(
        log,
        {
            "event": "placed" if ok else "place_failed_verification",
            "monitor": {k: monitor.get(k) for k in ("id", "name", "x", "y", "width", "height", "focused", "activeWorkspace")},
            "target_rect": [target_x, target_y, target_w, target_h],
            "before": before,
            "after": window_summary(after),
            "commands": results,
        },
    )
    return bool(ok)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--class", dest="window_class", default=TARGET_CLASS)
    parser.add_argument("--monitor", default="window", help="window, focused, monitor id, or monitor name. Default: window's own Hyprland monitor")
    parser.add_argument("--workspace", default="window", help="window, monitor, none, or explicit workspace id. Default: window's workspace")
    parser.add_argument("--width", type=int, default=1280)
    parser.add_argument("--height", type=int, default=720)
    parser.add_argument("--duration", type=float, default=0.0, help="seconds to keep correcting; 0 means one attempt")
    parser.add_argument("--interval", type=float, default=0.25)
    parser.add_argument("--always", action="store_true", help="reapply placement even if the target is already visible")
    parser.add_argument("--focus", action="store_true", help="focus the target after placement; off by default to avoid active-monitor churn")
    parser.add_argument("--log", type=Path)
    args = parser.parse_args()

    deadline = time.monotonic() + max(args.duration, 0.0)
    saw_success = False
    while True:
        try:
            saw_success = place_once(args, args.log) or saw_success
        except Exception as exc:  # Keep the helper diagnostic instead of silently dying during startup.
            emit(args.log, {"event": "exception", "error": repr(exc)})
        if args.duration <= 0 or time.monotonic() >= deadline:
            break
        time.sleep(max(args.interval, 0.05))
    return 0 if saw_success else 1


if __name__ == "__main__":
    raise SystemExit(main())
