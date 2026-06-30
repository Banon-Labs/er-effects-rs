#!/usr/bin/env python3
"""Summarize target-only Elden Ring window-placement smoke artifacts."""
from __future__ import annotations

import json
import subprocess
import sys
from pathlib import Path
from typing import Any

FILES = [
    "launcher-wrapper.out",
    "capture-and-teardown.out",
    "wf-capture.out",
    "wf-recorder-request.json",
    "wf-recorder-result.json",
    "recording-not-started.json",
    "hypr-window-placer.jsonl",
    "target-window-observer.jsonl",
]


def load_observer(path: Path) -> list[dict[str, Any]]:
    rows: list[dict[str, Any]] = []
    if not path.exists():
        return rows
    for line in path.read_text(encoding="utf-8", errors="replace").splitlines():
        try:
            rows.append(json.loads(line))
        except json.JSONDecodeError:
            pass
    return rows


def first_window(row: dict[str, Any]) -> dict[str, Any] | None:
    windows = row.get("windows") or []
    return windows[0] if windows else None


def main() -> int:
    if len(sys.argv) != 2:
        print("usage: summarize-er-window-placement-smoke.py ARTIFACT_DIR", file=sys.stderr)
        return 2
    art = Path(sys.argv[1])
    print(f"artifact={art}")
    for name in FILES:
        path = art / name
        size = path.stat().st_size if path.exists() else 0
        print(f"{name}: exists={path.exists()} size={size}")

    rows = load_observer(art / "target-window-observer.jsonl")
    wins = [w for row in rows if (w := first_window(row))]
    ats = [tuple(w.get("at") or []) for w in wins]
    sizes = [tuple(w.get("size") or []) for w in wins]
    print(f"observer_samples={len(rows)}")
    print(f"window_samples={len(wins)}")
    print(f"unique_at={sorted(set(ats))[:20]}")
    print(f"unique_size={sorted(set(sizes))[:20]}")
    print(f"negative_x_samples={sum(1 for w in wins if (w.get('at') or [0])[0] < 0)}")

    if rows:
        t0 = rows[0].get("t") or 0
        prev: tuple[Any, ...] | None = None
        for idx, row in enumerate(rows, 1):
            w = first_window(row)
            if not w:
                continue
            workspace = w.get("workspace") or {}
            key = (
                tuple(w.get("at") or []),
                tuple(w.get("size") or []),
                w.get("focusHistoryID"),
                w.get("floating"),
                w.get("monitor"),
                workspace.get("id"),
            )
            if key != prev:
                print(f"transition line={idx} t={round((row.get('t') or 0) - t0, 3)} key={key} pid={w.get('pid')}")
                prev = key

    req = art / "wf-recorder-request.json"
    if req.exists():
        data = json.loads(req.read_text(encoding="utf-8", errors="replace"))
        w = data.get("window") or {}
        print(f"recorder_window_at={w.get('at')} size={w.get('size')} focus={w.get('focusHistoryID')} floating={w.get('floating')}")
    else:
        not_started = art / "recording-not-started.json"
        if not_started.exists():
            print("recording_not_started=" + not_started.read_text(encoding="utf-8", errors="replace").strip())

    launcher = art / "launcher-wrapper.out"
    if launcher.exists():
        for line in launcher.read_text(encoding="utf-8", errors="replace").splitlines():
            if any(token in line for token in ("graphics-config:", "hypr-place:", "render:", "fatal", "error")):
                print("launcher: " + line)

    result = subprocess.run(["pgrep", "-x", "eldenring.exe"], text=True, capture_output=True)
    print(f"eldenring.exe: rc={result.returncode} pids={result.stdout.strip()}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
