#!/usr/bin/env python3
"""Full-resolution screenshot of ONLY the Elden Ring game window.

Like scripts/capture-er-window.py but keeps the capture at FULL resolution (no
854x480 downscale / quality-35 jpg), so on-screen menu/title text is legible.

Selects strictly the window owned by the running eldenring.exe PID (preferring
class steam_app_1245620). Validates mapped + sane geometry, focuses/raises it,
then grim-captures that exact region. Never enumerates or captures other windows
/ the desktop. Writes a .txt note and takes NO screenshot if the game window
can't be safely validated.

Usage: capture-er-window-fullres.py <out.png>
Exit 0 always (best-effort evidence; never fails the caller).
"""
from __future__ import annotations
import json, shutil, subprocess, sys
from pathlib import Path

WINDOW_CLASS = "steam_app_1245620"


def er_pids() -> set[int]:
    try:
        out = subprocess.run(["pgrep", "-x", "eldenring.exe"], text=True,
                             capture_output=True, timeout=10).stdout
        return {int(x) for x in out.split()}
    except Exception:
        return set()


def hypr_clients(hyprctl: str) -> list[dict]:
    try:
        out = subprocess.run([hyprctl, "clients", "-j"], text=True,
                             capture_output=True, timeout=10).stdout
        return [c for c in json.loads(out) if isinstance(c, dict)]
    except Exception:
        return []


def find_game_window(clients: list[dict], pids: set[int]) -> dict | None:
    by_class = [c for c in clients if str(c.get("class") or "") == WINDOW_CLASS]
    if by_class:
        return by_class[0]
    by_pid = [c for c in clients if c.get("pid") in pids]
    return by_pid[0] if by_pid else None


def problems(w: dict) -> list[str]:
    p = []
    if w.get("mapped") is False:
        p.append("unmapped")
    if w.get("hidden") is True:
        p.append("hidden")
    at, size = w.get("at") or [], w.get("size") or []
    if len(at) != 2 or len(size) != 2 or int(size[0] or 0) <= 0 or int(size[1] or 0) <= 0:
        p.append("bad_geometry")
    return p


def summ(w: dict) -> dict:
    return {k: w.get(k) for k in ("class", "pid", "at", "size", "mapped", "hidden", "focusHistoryID", "fullscreen")}


def main() -> int:
    out = Path(sys.argv[1])
    note = out.with_suffix(".txt")
    hyprctl, grim = shutil.which("hyprctl"), shutil.which("grim")
    if not hyprctl or not grim:
        note.write_text(f"capture skipped: hyprctl={hyprctl} grim={grim}\n"); return 0
    pids = er_pids()
    if not pids:
        note.write_text("capture fail-closed: no eldenring.exe process\n"); return 0
    w = find_game_window(hypr_clients(hyprctl), pids)
    if w is None:
        note.write_text(f"capture fail-closed: no game window for pids={pids}\n"); return 0
    addr = w.get("address")
    ws = w.get("workspace")
    ws_id = ws.get("id") if isinstance(ws, dict) else ws
    for _ in range(24):
        try:
            if ws_id is not None:
                subprocess.run([hyprctl, "dispatch", "workspace", str(ws_id)], capture_output=True, timeout=10)
            if addr:
                subprocess.run([hyprctl, "dispatch", "focuswindow", f"address:{addr}"], capture_output=True, timeout=10)
                subprocess.run([hyprctl, "dispatch", "alterzorder", f"top,address:{addr}"], capture_output=True, timeout=10)
        except Exception:
            pass
        w2 = find_game_window(hypr_clients(hyprctl), pids)
        if w2:
            w = w2
            fh = w.get("focusHistoryID")
            if fh is not None and int(fh) == 0:
                break
    probs = problems(w)
    if probs:
        note.write_text(f"capture fail-closed: window unsafe {probs} {summ(w)}\n"); return 0
    at, size = w["at"], w["size"]
    geom = f"{int(at[0])},{int(at[1])} {int(size[0])}x{int(size[1])}"
    try:
        rc = subprocess.run([grim, "-g", geom, str(out)], text=True, capture_output=True, timeout=15)
    except Exception as exc:
        note.write_text(f"capture failed: grim {exc}\n"); return 0
    if rc.returncode != 0 or not out.exists():
        note.write_text(f"capture failed: grim rc={rc.returncode} {rc.stderr.strip()}\n"); return 0
    note.write_text(f"captured {geom} -> {out.name} ({summ(w)})\n")
    print(f"captured {out} geom={geom} class={w.get('class')} pid={w.get('pid')}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
