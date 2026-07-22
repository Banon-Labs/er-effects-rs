#!/usr/bin/env python3
"""Test H-B (single-core contention) vs H-A (real load-completion bug) for the load2/load3 20fps.

Reads er-telemetry-timeseries.jsonl (needs the per-core CPU fields oracle_core_max_busy /
oracle_cores_saturated / oracle_proc_cpu_cores added 2026-07-22). Splits samples by the flip
regime -- fixed_spf 0.0167 (60fps target, load complete) vs 0.05 (20fps loading cap engaged) --
and reports the CPU picture in each. The decisive question: during the CAPPED (0.05) periods,
is a core pinned ~100% (=> H-B contention starving the single-threaded asset load) or is no core
saturated while the load still stalls (=> H-A real completion bug)?

Usage: python3 scripts/analyze-core-contention.py <timeseries.jsonl-or-artifact-dir>
"""
from __future__ import annotations

import json
import os
import statistics
import sys


def load(path: str):
    if os.path.isdir(path):
        for c in ("er-telemetry-timeseries.jsonl", "vanilla-userdrive-timeseries.jsonl"):
            p = os.path.join(path, c)
            if os.path.exists(p):
                path = p
                break
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


def num(v, d=-1.0):
    try:
        return float(v)
    except (TypeError, ValueError):
        return d


def summarize(label, rows):
    if not rows:
        print(f"\n## {label}: no samples")
        return
    fps = [1.0 / num(r.get("oracle_flip_task_delta")) for r in rows if num(r.get("oracle_flip_task_delta")) > 0.001]
    maxc = [num(r.get("oracle_core_max_busy")) for r in rows if num(r.get("oracle_core_max_busy")) >= 0]
    sat = [num(r.get("oracle_cores_saturated")) for r in rows if num(r.get("oracle_cores_saturated")) >= 0]
    proc = [num(r.get("oracle_proc_cpu_cores")) for r in rows if num(r.get("oracle_proc_cpu_cores")) >= 0]
    ncores = next((int(num(r.get("oracle_ncores"))) for r in rows if num(r.get("oracle_ncores")) > 0), 0)
    print(f"\n## {label}   n={len(rows)}  ncores={ncores}")
    if fps:
        print(f"   actual fps: mean={statistics.mean(fps):.0f} min={min(fps):.0f} max={max(fps):.0f}")
    if maxc:
        pinned = sum(1 for c in maxc if c >= 95)
        print(f"   busiest-core %: mean={statistics.mean(maxc):.0f} min={min(maxc):.0f} max={max(maxc):.0f}  |  frames w/ a core >=95%: {pinned}/{len(maxc)} ({100*pinned/len(maxc):.0f}%)")
    if sat:
        print(f"   cores saturated (>85%): mean={statistics.mean(sat):.1f} max={int(max(sat))}")
    if proc:
        print(f"   ER process CPU (core-equiv): mean={statistics.mean(proc):.2f} max={max(proc):.2f}")


def main() -> int:
    if len(sys.argv) < 2:
        print(__doc__)
        return 2
    rows, path = load(sys.argv[1])
    print(f"# {path}  ({len(rows)} samples)")
    if not rows:
        print("no samples -- telemetry DLL loaded + core fields present?")
        return 1
    if not any("oracle_core_max_busy" in r for r in rows):
        print("!! no oracle_core_max_busy field -- this run predates the per-core CPU capture; rebuild+rerun.")
        return 1

    def spf(r):
        return num(r.get("oracle_flip_fixed_spf"))

    capped = [r for r in rows if 0.04 < spf(r) < 0.06]      # 0.05 = 20fps loading cap engaged
    target60 = [r for r in rows if 0.015 < spf(r) < 0.02]   # 0.0167 = 60fps target
    summarize("CAPPED @ fixed_spf~0.05 (20fps loading cap ON -- load in progress/stuck)", capped)
    summarize("TARGET @ fixed_spf~0.0167 (60fps -- load complete)", target60)

    print("\n## VERDICT")
    if not capped:
        print("   No 0.05 cap samples in this run -- either load completed throughout (like vanilla) or the")
        print("   run didn't reach a stalling load. No H-A/H-B call possible from this capture.")
        return 0
    maxc = [num(r.get("oracle_core_max_busy")) for r in capped if num(r.get("oracle_core_max_busy")) >= 0]
    if maxc:
        pinned_frac = sum(1 for c in maxc if c >= 95) / len(maxc)
        if pinned_frac >= 0.6:
            print(f"   During the 20fps cap, a core is pinned >=95% {100*pinned_frac:.0f}% of frames")
            print("   => H-B (single-core CONTENTION) is a live factor -- the load is being starved, not")
            print("      necessarily broken. Re-test with the contending work (parallel cargo/agents) stopped.")
        else:
            print(f"   During the 20fps cap, NO core is pinned (busiest >=95% only {100*pinned_frac:.0f}% of frames)")
            print("   => H-A (real completion bug) -- the load stalls with CPU headroom to spare; contention")
            print("      is not the cause; the reload genuinely isn't finishing.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
