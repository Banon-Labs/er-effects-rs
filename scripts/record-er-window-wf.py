#!/usr/bin/env python3
"""Record the validated Elden Ring Hyprland window with wf-recorder.

This is target-window-only and geometry-derived. It waits for the runtime probe's
hypr-window-placer.jsonl proof, verifies the live steam_app_1245620 address and geometry match the
placer `after` record, then records that exact arbitrary location/resolution with wf-recorder.
"""
from __future__ import annotations

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


def find_window() -> dict[str, Any] | None:
    hyprctl = shutil.which("hyprctl")
    if not hyprctl:
        return None
    try:
        clients = json.loads(run([hyprctl, "clients", "-j"], timeout=10).stdout)
    except Exception:
        return None
    for c in clients if isinstance(clients, list) else []:
        if c.get("class") == WINDOW_CLASS:
            return c
    return None


def sane_window(w: dict[str, Any] | None) -> bool:
    if not w or w.get("mapped") is False or w.get("hidden") is True:
        return False
    at = w.get("at") or []
    size = w.get("size") or []
    if len(at) != 2 or len(size) != 2:
        return False
    x, y = int(at[0]), int(at[1])
    sx, sy = int(size[0]), int(size[1])
    return x >= 0 and y >= 0 and sx >= 320 and sy >= 240


def latest_placer_after(art: Path) -> dict[str, Any] | None:
    p = art / "hypr-window-placer.jsonl"
    if not p.exists():
        return None
    latest = None
    for line in p.read_text(errors="replace").splitlines():
        try:
            obj = json.loads(line)
        except Exception:
            continue
        if obj.get("event") == "placed" and isinstance(obj.get("after"), dict):
            latest = obj
    return latest


def wait_for_placed_window(art: Path, timeout_s: float = 12.0) -> tuple[dict[str, Any], dict[str, Any]]:
    deadline = time.time() + timeout_s
    last_reason = "not started"
    while time.time() < deadline:
        placed = latest_placer_after(art)
        w = find_window()
        if placed and sane_window(w):
            assert w is not None
            after = placed["after"]
            at = w.get("at") or []
            size = w.get("size") or []
            pat = after.get("at") or []
            psize = after.get("size") or []
            if w.get("address") == after.get("address") and list(map(int, at)) == list(map(int, pat)) and list(map(int, size)) == list(map(int, psize)):
                return w, placed
            last_reason = f"current addr/geom {w.get('address')} {at} {size} != placer {after.get('address')} {pat} {psize}"
        elif placed:
            last_reason = f"placed exists but current window is not sane: {w}"
        else:
            last_reason = "waiting for hypr-window-placer placed event"
        time.sleep(0.05)
    raise SystemExit(f"no stable placed ER window: {last_reason}")


def main() -> int:
    if len(sys.argv) != 4:
        print("usage: record-er-window-wf.py <artifact-dir> <seconds> <fps>", file=sys.stderr)
        return 2
    art = Path(sys.argv[1])
    seconds = float(sys.argv[2])
    fps = float(sys.argv[3])
    wf = shutil.which("wf-recorder")
    if not wf:
        raise SystemExit("missing wf-recorder")
    w, placed = wait_for_placed_window(art)
    at = list(map(int, w["at"]))
    size = list(map(int, w["size"]))
    geom = f"{at[0]},{at[1]} {size[0]}x{size[1]}"
    out = art / f"wf-{fps:g}fps.mkv"
    meta = {
        "window": {k: w.get(k) for k in ("address", "class", "at", "size", "mapped", "hidden", "focusHistoryID", "fullscreen", "workspace", "monitor")},
        "placer_record": placed,
        "geometry": geom,
        "seconds": seconds,
        "fps": fps,
        "output": str(out),
    }
    (art / "wf-recorder-request.json").write_text(json.dumps(meta, indent=2))
    cmd = [wf, "-g", geom, "-r", str(fps), "-D", "-f", str(out)]
    proc = subprocess.Popen(cmd, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
    time.sleep(seconds)
    proc.terminate()
    try:
        stdout, stderr = proc.communicate(timeout=5)
    except subprocess.TimeoutExpired:
        proc.kill()
        stdout, stderr = proc.communicate()
    result = {"cmd": cmd, "returncode": proc.returncode, "stdout": stdout, "stderr": stderr, "output_exists": out.exists(), "output_size": out.stat().st_size if out.exists() else 0}
    (art / "wf-recorder-result.json").write_text(json.dumps(result, indent=2))
    print(json.dumps({"done": True, **result, "geometry": geom}))
    return 0 if out.exists() and out.stat().st_size > 0 else 1


if __name__ == "__main__":
    raise SystemExit(main())
