#!/usr/bin/env python3
"""Switch-reload framerate-parity delta (goal docs/goals/switch-reload-framerate-parity-acceptance.md).

Implements the precise §3/§4 measurement the coarse analyze-reload-fps-oracle-diff.py approximates:

  §3.1 steady-state window W(epoch) = samples from T+10s to T+30s after that epoch first reaches
       WORLD-STABLE (oracle_player_present AND oracle_play_time_live truthy for >=3 consecutive
       samples). T is the t_ms of that first world-stable sample.
  §3.2 frame_ms(epoch) = median over W of 1000/oracle_fps  (cross-checked vs qpc_delta_us/1000 and
       the DLL's own oracle_frame_ms).
  §3.3 gpu_frame_us(epoch) = median over W of oracle_gpu_frame_us (the injected D3D12 timestamp pair;
       0s dropped as pre-production). Prints samples/state so a 0 is attributable.
  §4.1 D_frame  = frame_ms(reload epoch)     - frame_ms(first-load epoch)
  §4.2 D_gpu    = gpu_frame_us(reload epoch)  - gpu_frame_us(first-load epoch)   (reported in ms)

The confound-controlled cross-run quantity is Delta = D_mod - D_van; run this on the mod run and the
vanilla run and subtract (this script reports each run's D; a --van <D_frame_ms> arg does the subtraction).

AC-1 (oracle valid, §3.3): across the run's distinct fps levels (epochs), gpu_frame_us must move
MONOTONICALLY OPPOSITE to oracle_fps -- printed as the monotonicity check.

Usage:
  python3 scripts/gpu-oracle-delta.py <telemetry-timeseries.jsonl> [--first-epoch N] [--reload-epoch M]
  python3 scripts/gpu-oracle-delta.py <mod.jsonl> --van-dframe 1.23 --van-dgpu 1.10   # cross-run Delta
"""
from __future__ import annotations

import argparse
import json
import statistics as st
from pathlib import Path

WORLD_STABLE_CONSEC = 3  # §3.1: player_present AND play_time_live for >=3 consecutive samples
W_START_MS = 10_000  # §3.1: window starts T+10s
W_END_MS = 30_000  # §3.1: window ends T+30s


def _f(v):
    try:
        return float(v)
    except (TypeError, ValueError):
        return None


def _truthy(v) -> bool:
    return v in (1, True, "1", "true", "True") or (isinstance(v, (int, float)) and v != 0)


def _summ(xs):
    xs = [x for x in xs if x is not None]
    if not xs:
        return None
    return {
        "n": len(xs),
        "median": st.median(xs),
        "mean": sum(xs) / len(xs),
        "sd": (st.pstdev(xs) if len(xs) > 1 else 0.0),
        "min": min(xs),
        "max": max(xs),
    }


def load_rows(path: Path) -> list[dict]:
    rows = []
    for line in path.read_text(encoding="utf-8", errors="replace").splitlines():
        line = line.strip()
        if not line:
            continue
        try:
            rows.append(json.loads(line))
        except json.JSONDecodeError:
            continue
    return rows


def world_stable_t(epoch_rows: list[dict]):
    """t_ms of the first sample after which player_present AND play_time_live are truthy for
    WORLD_STABLE_CONSEC consecutive samples; None if never reached."""
    consec = 0
    for r in epoch_rows:
        stable = _truthy(r.get("oracle_player_present")) and _truthy(
            r.get("oracle_play_time_live")
        )
        if stable:
            consec += 1
            if consec >= WORLD_STABLE_CONSEC:
                # T = the first of the consecutive run
                idx = epoch_rows.index(r) - (WORLD_STABLE_CONSEC - 1)
                return _f(epoch_rows[max(0, idx)].get("t_ms"))
        else:
            consec = 0
    return None


def window_rows(epoch_rows: list[dict], t0: float) -> list[dict]:
    lo, hi = t0 + W_START_MS, t0 + W_END_MS
    return [r for r in epoch_rows if (_f(r.get("t_ms")) or -1) >= lo and (_f(r.get("t_ms")) or -1) <= hi]


def epoch_metrics(win: list[dict]) -> dict:
    fps = _summ([_f(r.get("oracle_fps")) for r in win])
    frame_ms = _summ([1000.0 / f for r in win if (f := _f(r.get("oracle_fps"))) and f > 0])
    frame_ms_dll = _summ([_f(r.get("oracle_frame_ms")) for r in win])
    qpc_ms = _summ([q / 1000.0 for r in win if (q := _f(r.get("oracle_present_qpc_delta_us")))])
    gpu_us = _summ([g for r in win if (g := _f(r.get("oracle_gpu_frame_us")))])  # drops 0s
    gpu_samp = max([_f(r.get("oracle_gpu_frame_samples")) or 0 for r in win], default=0)
    gpu_state = max([_f(r.get("oracle_gpu_frame_state")) or 0 for r in win], default=0)
    return dict(
        fps=fps,
        frame_ms=frame_ms,
        frame_ms_dll=frame_ms_dll,
        qpc_ms=qpc_ms,
        gpu_us=gpu_us,
        gpu_samples=gpu_samp,
        gpu_state=gpu_state,
        n=len(win),
    )


def fmt(s, unit="", scale=1.0):
    if not s:
        return "n/a"
    return f"med={s['median']*scale:.3f}{unit} sd={s['sd']*scale:.3f} n={s['n']}"


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("timeseries", type=Path)
    ap.add_argument("--first-epoch", type=int, default=None, help="first-load epoch (default: min)")
    ap.add_argument("--reload-epoch", type=int, default=None, help="reload epoch (default: max)")
    ap.add_argument("--van-dframe", type=float, default=None, help="vanilla D_frame_ms for cross-run Delta")
    ap.add_argument("--van-dgpu", type=float, default=None, help="vanilla D_gpu_ms for cross-run Delta")
    args = ap.parse_args()

    rows = load_rows(args.timeseries)
    if not rows:
        print(f"no rows in {args.timeseries}")
        return 1
    epochs = sorted({int(_f(r.get("oracle_current_load_epoch")) or 0) for r in rows})
    print(f"file: {args.timeseries}")
    print(f"rows: {len(rows)}   epochs: {epochs}   window: T+{W_START_MS//1000}s..T+{W_END_MS//1000}s\n")

    per = {}
    for ep in epochs:
        er = [r for r in rows if int(_f(r.get("oracle_current_load_epoch")) or 0) == ep]
        t0 = world_stable_t(er)
        if t0 is None:
            print(f"== epoch {ep} (load{ep+1}): NEVER world-stable (no player_present+play_time_live x{WORLD_STABLE_CONSEC}) ==\n")
            per[ep] = None
            continue
        win = window_rows(er, t0)
        m = epoch_metrics(win)
        per[ep] = m
        print(f"== epoch {ep} (load{ep+1}): world-stable @ t={t0/1000:.1f}s, W has {m['n']} samples ==")
        print(f"   fps        {fmt(m['fps'])}")
        print(f"   frame_ms   {fmt(m['frame_ms'],'ms')}   (dll_frame_ms {fmt(m['frame_ms_dll'],'ms')}; qpc {fmt(m['qpc_ms'],'ms')})")
        if m["gpu_us"]:
            print(f"   gpu_frame  {fmt(m['gpu_us'],'ms',0.001)}   samples={int(m['gpu_samples'])} state={int(m['gpu_state'])}")
        else:
            print(f"   gpu_frame  NO samples (state={int(m['gpu_state'])} samples={int(m['gpu_samples'])}) -- oracle not live in W")
        print()

    # AC-1 monotonicity: gpu_frame_us should move opposite to fps across epochs with data.
    pts = [
        (m["fps"]["median"], m["gpu_us"]["median"])
        for ep in epochs
        if (m := per.get(ep)) and m["fps"] and m["gpu_us"]
    ]
    print("== AC-1 monotonicity (gpu_frame_us vs fps, need opposite trend) ==")
    if len(pts) >= 2:
        pts_by_fps = sorted(pts, key=lambda p: p[0])
        gpus = [g for _, g in pts_by_fps]
        mono = all(gpus[i] >= gpus[i + 1] for i in range(len(gpus) - 1))
        for f, g in pts_by_fps:
            print(f"   fps={f:6.1f} -> gpu_frame={g/1000:.3f}ms")
        print(f"   MONOTONIC-OPPOSITE: {'PASS' if mono else 'FAIL'} ({len(pts)} fps levels)")
    else:
        print(f"   insufficient data ({len(pts)} usable epochs; need >=2, ideally >=3)")
    print()

    # §4 within-run D
    fe = args.first_epoch if args.first_epoch is not None else epochs[0]
    re_ = args.reload_epoch if args.reload_epoch is not None else epochs[-1]
    a, b = per.get(fe), per.get(re_)
    print(f"== §4 within-run D (reload epoch {re_} minus first-load epoch {fe}) ==")
    if a and b and a["frame_ms"] and b["frame_ms"]:
        d_frame = b["frame_ms"]["median"] - a["frame_ms"]["median"]
        print(f"   D_frame_ms = {b['frame_ms']['median']:.3f} - {a['frame_ms']['median']:.3f} = {d_frame:+.3f} ms")
        d_gpu = None
        if a["gpu_us"] and b["gpu_us"]:
            d_gpu = (b["gpu_us"]["median"] - a["gpu_us"]["median"]) / 1000.0
            print(f"   D_gpu_ms   = {b['gpu_us']['median']/1000:.3f} - {a['gpu_us']['median']/1000:.3f} = {d_gpu:+.3f} ms")
        else:
            print("   D_gpu_ms   = n/a (gpu oracle not live in one/both epochs)")
        if args.van_dframe is not None:
            delta = d_frame - args.van_dframe
            print(f"\n   §4 Delta_frame = D_mod - D_van = {d_frame:+.3f} - ({args.van_dframe:+.3f}) = {delta:+.3f} ms")
            print(f"   AC-2 (|Delta| > max(0.10ms, 2sigma)) => REAL divergence; else confound artifact.")
        if args.van_dgpu is not None and d_gpu is not None:
            print(f"   §4 Delta_gpu   = {d_gpu:+.3f} - ({args.van_dgpu:+.3f}) = {d_gpu - args.van_dgpu:+.3f} ms")
    else:
        print("   n/a (an epoch never reached world-stable)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
