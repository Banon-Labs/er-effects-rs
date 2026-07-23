#!/usr/bin/env python3
"""Per-load (epoch) frame-time decomposition for the samechar reload FPS investigation.

Splits telemetry-timeseries.jsonl by oracle_current_load_epoch and, for each load's
MOVABLE window, decomposes the frame budget:

    frame_ms  ==  game_task_us + build_driver_us + composite_us + present_call_us + residual

where `residual` (frame_ms minus the measured CPU components) is the flip/vsync/GPU-wait
portion. Comparing load1 vs load2/load3 names WHERE the reload dip's extra per-frame cost
lands: a CPU component that balloons => product per-frame CPU work; residual that balloons
=> present-stack / GPU / DXGI-side (the Windows-vs-Linux amplification suspect).

A "settled" view drops the first SETTLE frames of each movable window so the transient
asset-streaming overlap (which reproduces cross-OS) does not mask the persistent component.

Usage:
    python3 scripts/analyze-reload-fps-oracle-diff.py <telemetry-timeseries.jsonl> [--settle N]
"""
from __future__ import annotations

import argparse
import json
import statistics as st
from pathlib import Path

# per-frame CPU components (microseconds) that sum under frame_ms
COMPONENTS_US = [
    "oracle_game_task_us",
    "oracle_build_driver_us",
    "oracle_composite_us",
    "oracle_present_call_us",
]
CONTEXT = [
    "oracle_flip_mode_current",
    "oracle_flip_fixed_spf",
    "oracle_flip_dynamic_active",
    "oracle_flip_dynamic_fps_lock",
    "oracle_flip_vsync_interval",
    "oracle_now_loading",
]


def _f(v):
    try:
        return float(v)
    except (TypeError, ValueError):
        return None


def _truthy(v) -> bool:
    return v in (1, True, "1", "true", "True")


def _stats(xs):
    xs = [x for x in xs if x is not None]
    if not xs:
        return None
    return {
        "n": len(xs),
        "mean": st.mean(xs),
        "median": st.median(xs),
        "p95": sorted(xs)[min(len(xs) - 1, int(len(xs) * 0.95))],
        "max": max(xs),
    }


def analyze(path: Path, settle: int) -> None:
    rows = []
    for line in path.read_text(encoding="utf-8", errors="replace").splitlines():
        line = line.strip()
        if not line:
            continue
        try:
            rows.append(json.loads(line))
        except json.JSONDecodeError:
            continue

    epochs = sorted({int(_f(r.get("oracle_current_load_epoch")) or 0) for r in rows})
    print(f"file: {path}")
    print(f"rows: {len(rows)}   epochs (loads): {epochs}   settle-drop: {settle} frames\n")

    per_epoch = {}
    for ep in epochs:
        mov = [
            r
            for r in rows
            if int(_f(r.get("oracle_current_load_epoch")) or 0) == ep
            and _truthy(r.get("oracle_can_move"))
        ]
        settled = mov[settle:] if len(mov) > settle else mov
        per_epoch[ep] = (mov, settled)

        if not mov:
            print(f"== load{ep+1} (epoch {ep}): NO movable frames ==\n")
            continue

        frame_ms = _stats([_f(r.get("oracle_frame_ms")) for r in settled])
        fps = _stats([_f(r.get("oracle_fps")) for r in settled])
        comps = {k: _stats([_f(r.get(k)) for r in settled]) for k in COMPONENTS_US}
        # residual per frame = frame_ms - sum(components in ms)
        resid = []
        for r in settled:
            fm = _f(r.get("oracle_frame_ms"))
            if fm is None:
                continue
            csum = sum((_f(r.get(k)) or 0.0) for k in COMPONENTS_US) / 1000.0
            resid.append(fm - csum)
        resid_s = _stats(resid)

        print(f"== load{ep+1} (epoch {ep}): movable={len(mov)} settled={len(settled)} ==")
        if fps:
            print(f"   fps        mean={fps['mean']:6.1f} median={fps['median']:6.1f} min-p5~max n={fps['n']}")
        if frame_ms:
            print(f"   frame_ms   mean={frame_ms['mean']:6.2f} median={frame_ms['median']:6.2f} p95={frame_ms['p95']:6.2f} max={frame_ms['max']:6.2f}")
        print("   --- CPU components (ms) ---")
        for k in COMPONENTS_US:
            s = comps[k]
            if s:
                print(f"   {k[7:]:18s} mean={s['mean']/1000:7.3f} median={s['median']/1000:7.3f} p95={s['p95']/1000:7.3f} max={s['max']/1000:7.3f}")
        if resid_s:
            print(f"   {'residual(flip/gpu)':18s} mean={resid_s['mean']:7.3f} median={resid_s['median']:7.3f} p95={resid_s['p95']:7.3f} max={resid_s['max']:7.3f}")
        # context (last value in window)
        ctx = {k: settled[-1].get(k) for k in CONTEXT if k in settled[-1]}
        print(f"   ctx: {ctx}\n")

    # delta table load1 -> loadN
    if len(epochs) > 1 and per_epoch.get(epochs[0], (None, None))[1]:
        base = per_epoch[epochs[0]][1]
        base_frame = _stats([_f(r.get("oracle_frame_ms")) for r in base])
        print("== DELTA vs load1 (median ms) ==")
        for ep in epochs[1:]:
            cur = per_epoch[ep][1]
            if not cur:
                print(f"   load{ep+1}: no settled frames")
                continue
            cur_frame = _stats([_f(r.get("oracle_frame_ms")) for r in cur])
            if not (base_frame and cur_frame):
                continue
            dframe = cur_frame["median"] - base_frame["median"]
            print(f"   load{ep+1}: frame_ms {base_frame['median']:.2f} -> {cur_frame['median']:.2f}  (+{dframe:.2f}ms)")
            for k in COMPONENTS_US:
                b = _stats([_f(r.get(k)) for r in base])
                c = _stats([_f(r.get(k)) for r in cur])
                if b and c:
                    d = (c["median"] - b["median"]) / 1000.0
                    if abs(d) >= 0.05:
                        print(f"        {k[7:]:18s} {b['median']/1000:6.3f} -> {c['median']/1000:6.3f}  ({'+' if d>=0 else ''}{d:.3f}ms)")
            # residual delta
            def resid_of(rs):
                out = []
                for r in rs:
                    fm = _f(r.get("oracle_frame_ms"))
                    if fm is None:
                        continue
                    out.append(fm - sum((_f(r.get(k)) or 0.0) for k in COMPONENTS_US) / 1000.0)
                return _stats(out)
            rb, rc = resid_of(base), resid_of(cur)
            if rb and rc:
                print(f"        {'residual(flip/gpu)':18s} {rb['median']:6.3f} -> {rc['median']:6.3f}  ({'+' if rc['median']-rb['median']>=0 else ''}{rc['median']-rb['median']:.3f}ms)")


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("timeseries", type=Path)
    ap.add_argument("--settle", type=int, default=60, help="drop first N movable frames per load")
    args = ap.parse_args()
    analyze(args.timeseries, args.settle)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
