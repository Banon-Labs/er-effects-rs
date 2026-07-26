#!/usr/bin/env python3
"""Analyze a vanilla (telemetry-only, user-driven) reload FPS capture.

Reads vanilla-timeseries.jsonl (from run-vanilla-reload-fps.sh) and segments the
IN-WORLD (world-live) periods, split by the play_time PLATEAU during a System->Quit
->Continue reload (play_time is cumulative per character, so it does NOT reset on a
same-char reload -- it PAUSES while the world is torn down, then resumes). Reports
the game frame time (flip task_delta -> fps) per world-live period so period 0
(boot-continue) can be compared to period 1 (the reload). bd
USER-chose-vanilla-reload-comparison-2026-07-22.

Usage: python3 scripts/analyze-vanilla-reload-fps.py <artifact-dir-or-jsonl>
"""
from __future__ import annotations

import json
import os
import statistics
import sys


def load_rows(path: str):
    if os.path.isdir(path):
        for cand in ("er-telemetry-timeseries.jsonl", "vanilla-timeseries.jsonl"):
            p = os.path.join(path, cand)
            if os.path.exists(p):
                path = p
                break
    rows = []
    for line in open(path, encoding="utf-8", errors="replace").read().splitlines():
        line = line.strip()
        if not line:
            continue
        try:
            rows.append(json.loads(line))
        except json.JSONDecodeError:
            continue
    return rows, path


def fnum(v, d=-1.0):
    try:
        return float(v)
    except (TypeError, ValueError):
        return d


def main() -> int:
    if len(sys.argv) < 2:
        print(__doc__)
        return 2
    rows, path = load_rows(sys.argv[1])
    print(f"# {path}  ({len(rows)} samples)")
    if not rows:
        print("no samples -- did the game write er-telemetry-standalone.json? (telemetry DLL loaded?)")
        return 1

    # A sample is "world-live" if play_time advanced since the last sample. Group consecutive world-live
    # samples into periods; a run of >= PLATEAU_SAMPLES consecutive NON-rising play_time samples (the
    # world torn down during System->Quit->Continue, while the frame counter keeps ticking) splits them.
    # The DLL writes every 4th frame, so 8 samples ~= 32 frames of flat play_time = a real reload gap.
    PLATEAU_SAMPLES = 8
    periods: list[list[dict]] = []
    cur: list[dict] = []
    last_pt = None
    flat_run = 0
    for r in rows:
        pt = fnum(r.get("oracle_play_time_ms"), -1)
        rising = last_pt is not None and pt > last_pt
        if rising:
            if flat_run >= PLATEAU_SAMPLES and cur:
                periods.append(cur)
                cur = []
            cur.append(r)
            flat_run = 0
        else:
            flat_run += 1
        last_pt = pt
    if cur:
        periods.append(cur)

    if not periods:
        print("no world-live periods detected (play_time never advanced -- was the char in-world?).")
        # still dump the raw fps range for eyeballing
        fps_all = [fnum(r.get("fps")) for r in rows if fnum(r.get("fps")) > 0]
        if fps_all:
            print(f"raw fps over whole capture: mean={statistics.mean(fps_all):.0f} min={min(fps_all):.0f} max={max(fps_all):.0f} n={len(fps_all)}")
        return 0

    names = ["boot-continue (load1-equivalent)", "RELOAD (load2-equivalent)", "reload2", "reload3"]
    means = []
    for i, p in enumerate(periods):
        fps = [fnum(r.get("fps")) for r in p if fnum(r.get("fps")) > 0]
        spf = [fnum(r.get("oracle_flip_fixed_spf")) for r in p if fnum(r.get("oracle_flip_fixed_spf")) > 0]
        t0, t1 = fnum(p[0].get("oracle_standalone_ticks")), fnum(p[-1].get("oracle_standalone_ticks"))
        label = names[i] if i < len(names) else f"period{i}"
        if fps:
            m = statistics.mean(fps)
            means.append(m)
            print(
                f"\n## period {i}: {label}   t={t0:.0f}..{t1:.0f}s  n={len(fps)}\n"
                f"   fps: mean={m:.0f} min={min(fps):.0f} max={max(fps):.0f}   "
                f"fixed_spf={statistics.mean(spf):.4f} (~{1 / statistics.mean(spf):.0f}fps target)"
                if spf
                else f"\n## period {i}: {label}   t={t0:.0f}..{t1:.0f}s  n={len(fps)}\n   fps: mean={m:.0f} min={min(fps):.0f} max={max(fps):.0f}"
            )
        else:
            print(f"\n## period {i}: {label}   t={t0:.0f}..{t1:.0f}s   (no fps samples)")

    if len(means) >= 2:
        boot, reload = means[0], means[1]
        ratio = 100 * reload / boot if boot > 0 else 0
        print(
            f"\n## VERDICT: boot={boot:.0f}fps  reload={reload:.0f}fps ({ratio:.0f}% of boot)\n"
            f"   -> reload ~= boot => OUR reload path causes the DLL-run slowdown (fixable).\n"
            f"   -> reload << boot (like the product's 20 vs 50) => INHERENT to game reloads in this env."
        )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
