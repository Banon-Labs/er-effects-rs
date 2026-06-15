#!/usr/bin/env python3
"""Compute game-thread frame-task health from a runtime probe progress log.

Runtime probes (boot-then-poll) append lines of the form
    [t=<seconds>s procs=<n>] ... game_task_ticks=<k> ...
This derives `runtime_frame_task_hz` = delta(game_task_ticks)/delta(seconds)
between successive samples. A healthy Elden Ring title runs the recurring
FrameBegin task at ~60 Hz; a low value means a per-frame cost in the DLL is
crippling the game's frame rate (user-visible perturbation, a discard signal).

Usage: python3 .auto/frame_task_hz.py <progress.log> [more.log ...]
Emits METRIC runtime_frame_task_hz_min / _avg lines and a JSON summary.
"""
from __future__ import annotations

import json
import re
import sys
from pathlib import Path

SAMPLE_RE = re.compile(r"\[t=(\d+)s[^\]]*\].*?game_task_ticks=(\d+)")
HEALTHY_TITLE_HZ = 60.0


def hz_series(text: str) -> list[tuple[int, float]]:
    samples: list[tuple[int, int]] = []
    for line in text.splitlines():
        m = SAMPLE_RE.search(line)
        if m:
            samples.append((int(m.group(1)), int(m.group(2))))
    series: list[tuple[int, float]] = []
    for (t0, k0), (t1, k1) in zip(samples, samples[1:]):
        if t1 > t0 and k1 >= k0:
            series.append((t1, round((k1 - k0) / (t1 - t0), 1)))
    return series


def summarize(paths: list[Path]) -> dict[str, object]:
    series: list[tuple[int, float]] = []
    for path in paths:
        series.extend(hz_series(path.read_text(encoding="utf-8", errors="replace")))
    values = [hz for _, hz in series]
    if not values:
        return {"runtime_frame_task_hz_min": None, "runtime_frame_task_hz_avg": None, "samples": 0}
    return {
        "runtime_frame_task_hz_min": min(values),
        "runtime_frame_task_hz_avg": round(sum(values) / len(values), 1),
        "healthy_title_hz": HEALTHY_TITLE_HZ,
        "samples": len(values),
        "series": series,
    }


def main(argv: list[str]) -> int:
    if not argv:
        print("usage: frame_task_hz.py <progress.log> [...]", file=sys.stderr)
        return 2
    summary = summarize([Path(a) for a in argv])
    print(f"METRIC runtime_frame_task_hz_min={summary['runtime_frame_task_hz_min']}")
    print(f"METRIC runtime_frame_task_hz_avg={summary['runtime_frame_task_hz_avg']}")
    print(json.dumps(summary))
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
