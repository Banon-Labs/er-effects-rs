#!/usr/bin/env python3
"""Display the input-harness per-phase telemetry (er-input-harness-phases.jsonl).

VIEWER ONLY. The DECISION/comparison logic (vanilla-vs-product per-phase deltas,
"does this situation apply / is it useful for comparison") is the ORACLE dll's job
(bd ORACLE-dll-decides-reports-harness-drives-telemetry-gathers-...-2026-07-22). This
script just renders what the harness (driving) + telemetry (gathering) captured for a
SINGLE run, so a human/agent can read the per-phase timing + boundary semaphores that
the harness emitted for: startup, press_any_button, continue, wait_load_in, menu_flow,
quit_to_menu (and the reload continue). bd HARNESS-per-phase-telemetry-full-native-flow-2026-07-22.

Usage: python3 scripts/report-harness-phases.py <artifact-dir-or-jsonl>
"""
from __future__ import annotations

import json
import os
import sys


def load(path: str):
    if os.path.isdir(path):
        p = os.path.join(path, "er-input-harness-phases.jsonl")
        if os.path.exists(p):
            path = p
    rows = []
    if not os.path.exists(path):
        return rows, path
    for line in open(path, encoding="utf-8", errors="replace").read().splitlines():
        line = line.strip()
        if not line:
            continue
        try:
            rows.append(json.loads(line))
        except json.JSONDecodeError:
            continue
    return rows, path


def load_timeseries(phases_path: str):
    """Load the sibling telemetry timeseries (fps samples with oracle_tick_ms) so each phase window can
    be annotated with its mean fps. Returns a list of (tick_ms, fps) sorted by tick_ms, or []."""
    d = os.path.dirname(phases_path)
    p = os.path.join(d, "er-telemetry-timeseries.jsonl")
    samples = []
    if not os.path.exists(p):
        return samples
    for line in open(p, encoding="utf-8", errors="replace").read().splitlines():
        line = line.strip()
        if not line:
            continue
        try:
            r = json.loads(line)
        except json.JSONDecodeError:
            continue
        tick = r.get("oracle_tick_ms")
        spf = r.get("oracle_flip_task_delta")
        if isinstance(tick, (int, float)) and tick > 0 and isinstance(spf, (int, float)) and spf > 0:
            samples.append((float(tick), 1.0 / spf))
    samples.sort()
    return samples


def phase_fps(samples, start_ms, end_ms):
    """Mean fps of samples whose tick_ms falls in [start_ms, end_ms]."""
    if not isinstance(start_ms, (int, float)) or not isinstance(end_ms, (int, float)):
        return None
    fps = [f for (t, f) in samples if start_ms <= t <= end_ms]
    return sum(fps) / len(fps) if fps else None


def main() -> int:
    if len(sys.argv) < 2:
        print(__doc__)
        return 2
    rows, path = load(sys.argv[1])
    print(f"# {path}  ({len(rows)} phases)")
    if not rows:
        print("no phase telemetry -- did the harness write er-input-harness-phases.jsonl? (harness DLL loaded, phase-split build?)")
        return 1
    samples = load_timeseries(path)
    fps_note = f"  (fps from {len(samples)} aligned telemetry samples)" if samples else "  (no aligned telemetry timeseries -- oracle_tick_ms missing)"

    print(f"# per-phase timing + boundary semaphores{fps_note}")
    print("#   fixed_spf: 0.0500=20fps loading cap / 0.0167=60fps (the differential-loop metric)")
    hdr = f"{'idx':>3} {'phase':<22} {'outcome':<9} {'ms':>7} {'frames':>7} {'fps':>5} {'fixed_spf':>9} {'nowload':>7} {'load_fsm':>8} {'world':>5}"
    print(hdr)
    print("-" * len(hdr))
    total_ms = 0
    for r in rows:
        dur_ms = r.get("duration_ms", -1)
        if isinstance(dur_ms, (int, float)) and dur_ms >= 0:
            total_ms += dur_ms
        outcome = r.get("outcome", "?")
        mark = "" if outcome == "advanced" else "  <-- DERAILED"
        fps = phase_fps(samples, r.get("start_tick_ms"), r.get("end_tick_ms"))
        fps_s = f"{fps:>5.0f}" if fps is not None else f"{'-':>5}"
        spf = r.get("fixed_spf", None)
        spf_s = f"{spf:>9.4f}" if isinstance(spf, (int, float)) else f"{'-':>9}"
        print(
            f"{r.get('idx','?'):>3} {str(r.get('phase','?')):<22} {outcome:<9} "
            f"{dur_ms:>7} {r.get('duration_frames','?'):>7} {fps_s} {spf_s} "
            f"{r.get('now_loading','?'):>7} {r.get('load_fsm','?'):>8} {r.get('world_sim','?'):>5}{mark}"
        )
    print("-" * len(hdr))
    print(f"total driven time: {total_ms} ms ({total_ms/1000:.1f}s) across {len(rows)} phases")
    derailed = [r for r in rows if r.get("outcome") != "advanced"]
    if derailed:
        print(f"DERAILED phases: {', '.join(str(r.get('phase')) for r in derailed)}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
