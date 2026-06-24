#!/usr/bin/env python3
"""Low-quality screenshot of ONLY the Elden Ring window, for teardown/crash/trap evidence.

Privacy fail-closed (matches the runtime-probe rule + er-readiness-watch validation): captures only
an exact-class (steam_app_1245620), mapped, unhidden, focused/topmost (focusHistoryID==0) window
with sane geometry. grim -g captures a screen REGION, so if the ER window is not topmost the region
could contain other apps -- therefore we focus the window first, then re-validate, and if it still
isn't safe we write a .txt note and take NO screenshot (never the desktop / other windows).

Usage: capture-er-window.py <out.jpg>
Exit 0 always (best-effort evidence; never fails the caller's teardown).
"""
from __future__ import annotations

import json
import shutil
import subprocess
import time
from pathlib import Path
import sys

WINDOW_CLASS = "steam_app_1245620"


def hypr_clients(hyprctl: str) -> list[dict]:
    try:
        out = subprocess.run([hyprctl, "clients", "-j"], text=True, capture_output=True, timeout=10).stdout
        clients = json.loads(out)
        return [c for c in clients if isinstance(c, dict)]
    except Exception:
        return []


def find_er(clients: list[dict]) -> dict | None:
    for c in clients:
        if str(c.get("class") or "") == WINDOW_CLASS:
            return c
    return None


def problems(w: dict) -> list[str]:
    p: list[str] = []
    if w.get("mapped") is False:
        p.append("unmapped")
    if w.get("hidden") is True:
        p.append("hidden")
    fh = w.get("focusHistoryID")
    if fh is None:
        p.append("focus_unknown")
    elif int(fh) != 0:  # 0 == most-recently-focused/topmost (do NOT use `or` -- 0 is falsy)
        p.append("not_focused")
    at, size = w.get("at") or [], w.get("size") or []
    if len(at) != 2 or len(size) != 2 or int(size[0] or 0) <= 0 or int(size[1] or 0) <= 0:
        p.append("bad_geometry")
    return p


def summary(w: dict) -> dict:
    return {k: w.get(k) for k in ("class", "at", "size", "mapped", "hidden", "focusHistoryID", "fullscreen")}


def main() -> int:
    out = Path(sys.argv[1]) if len(sys.argv) > 1 else Path("er-window.jpg")
    note = out.with_suffix(".txt")
    hyprctl = shutil.which("hyprctl")
    grim = shutil.which("grim")
    if not hyprctl or not grim:
        note.write_text(f"capture skipped: missing tool hyprctl={hyprctl} grim={grim}\n")
        return 0

    w = find_er(hypr_clients(hyprctl))
    if w is None:
        note.write_text(f"capture fail-closed: no window class={WINDOW_CLASS} (game gone/crashed)\n")
        return 0

    # Focus the ER window so the grim screen-region is guaranteed to be the game, then re-validate.
    # Switch to its workspace first (grim captures the visible output; the window must be on it),
    # focus + raise to top, and retry -- a single focuswindow often doesn't win focus on teardown.
    addr = w.get("address")
    ws = w.get("workspace")
    ws_id = ws.get("id") if isinstance(ws, dict) else ws
    # Re-query after each focus dispatch until the window reports topmost (focusHistoryID == 0),
    # bounded by a fixed attempt count. Each hyprctl subprocess call is synchronous (timeout=10) and
    # the re-query itself spawns hyprctl, so the loop paces on real IPC latency -- no sleep needed.
    for _attempt in range(24):
        try:
            if ws_id is not None:
                subprocess.run([hyprctl, "dispatch", "workspace", str(ws_id)], capture_output=True, timeout=10)
            if addr:
                subprocess.run([hyprctl, "dispatch", "focuswindow", f"address:{addr}"], capture_output=True, timeout=10)
                subprocess.run([hyprctl, "dispatch", "alterzorder", f"top,address:{addr}"], capture_output=True, timeout=10)
        except Exception:
            pass
        w2 = find_er(hypr_clients(hyprctl))
        if w2:
            w = w2
            fh = w.get("focusHistoryID")
            if fh is not None and int(fh) == 0:
                break

    probs = problems(w)
    if probs:
        note.write_text(f"capture fail-closed: ER window unsafe {probs} window={summary(w)}\n")
        return 0

    at, size = w["at"], w["size"]
    geom = f"{int(at[0])},{int(at[1])} {int(size[0])}x{int(size[1])}"
    png = out.with_suffix(".png")
    try:
        rc = subprocess.run([grim, "-g", geom, str(png)], text=True, capture_output=True, timeout=15)
    except Exception as exc:
        note.write_text(f"capture failed: grim error {exc}\n")
        return 0
    if rc.returncode != 0 or not png.exists():
        note.write_text(f"capture failed: grim rc={rc.returncode} stderr={rc.stderr.strip()}\n")
        return 0

    # Downscale to a LOW-QUALITY jpg (small artifact); keep the png if no imagemagick.
    magick = shutil.which("magick") or shutil.which("convert")
    target = png
    if magick:
        try:
            r = subprocess.run([magick, str(png), "-resize", "854x480>", "-quality", "35", str(out)],
                               capture_output=True, timeout=20)
            if r.returncode == 0 and out.exists():
                png.unlink(missing_ok=True)
                target = out
        except Exception:
            pass
    note.write_text(f"captured ER window class={WINDOW_CLASS} geom={geom} -> {target.name} ({summary(w)})\n")
    print(f"capture-er-window: {target} geom={geom}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
