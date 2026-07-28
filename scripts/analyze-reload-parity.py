#!/usr/bin/env python3
"""Score a same-char reload run for FRAMERATE PARITY (goal AC-2/AC-3): is the switch
reload's settled in-world framerate the same as the first load's?

Reads a runtime-probe artifact dir's telemetry-timeseries.jsonl (or a jsonl path directly),
segments rows by load epoch (fresh_deser_count), keeps only SETTLED in-world rows
(player_present), and reports per-epoch median refresh_per_present / qpc_delta / fps plus the
WITHIN-RUN delta load2-minus-load1 -- the confound-free AC-2 metric (same run, same env, so the
env baseline cancels). Verdict PARITY iff |load2 - load1| refresh_per_present <= NOISE_VBLANK.

Usage:
    python3 scripts/analyze-reload-parity.py <artifact-dir-or-timeseries.jsonl>

Why within-run delta: absolute fps is env/scene-bound (angrE renders ~20-30fps on BOTH loads);
the reload BUG is load2 rendering HEAVIER than load1 (the in-place reload left render globals
un-reset). Parity = load2 settled == load1 settled. bd
user-clue-angre-load2-dip-sustained-at-idle-persistent-render-state-not-transient-2026-07-23.
"""
from __future__ import annotations

import json
import statistics
import sys
from pathlib import Path

NOISE_VBLANK = 0.5  # |load2-load1| refresh/present within this = parity (sub-half-vblank = noise)

EPOCH_KEYS = (
    "fresh_deser_count",
    "oracle_fresh_deser_count",
    "oracle_switch_reload_committed",
    "oracle_current_load_epoch",
)
PRESENT_KEYS = ("oracle_switch_player_present", "oracle_player_present")
RP_KEY = "oracle_present_refresh_per_present_x100"
QPC_KEY = "oracle_present_qpc_delta_us"
FPS_KEY = "oracle_fps"


def _epoch(row: dict):
    for k in EPOCH_KEYS:
        v = row.get(k)
        if v is not None:
            return v
    return None


def _settled(row: dict) -> bool:
    for k in PRESENT_KEYS:
        v = row.get(k)
        if v is not None:
            return v == 1 or v is True
    return False


def _median(xs: list[float]):
    return statistics.median(xs) if xs else None


def main() -> int:
    if len(sys.argv) != 2:
        print(__doc__)
        return 2
    arg = Path(sys.argv[1])
    ts = arg / "telemetry-timeseries.jsonl" if arg.is_dir() else arg
    if not ts.exists():
        print(f"ERROR: no timeseries at {ts}", file=sys.stderr)
        return 2

    by_epoch: dict[object, dict[str, list[float]]] = {}
    for line in ts.read_text(encoding="utf-8", errors="replace").splitlines():
        line = line.strip()
        if not line:
            continue
        try:
            row = json.loads(line)
        except json.JSONDecodeError:
            continue
        if not _settled(row):
            continue
        e = _epoch(row)
        if e is None:
            continue
        bucket = by_epoch.setdefault(e, {"rp": [], "qpc": [], "fps": []})
        for key, field in (("rp", RP_KEY), ("qpc", QPC_KEY), ("fps", FPS_KEY)):
            v = row.get(field)
            if isinstance(v, (int, float)):
                bucket[key].append(float(v))

    epochs = sorted(by_epoch.keys(), key=lambda x: (x is None, x))
    print(f"# {ts}")
    if not epochs:
        print("no settled (player_present) rows found -- run never reached a movable in-world window")
        return 1

    label = {0: "load1(firstload)", 1: "load2(reload)", 2: "load3(reload2)"}
    rp_by_epoch: dict[object, float] = {}
    for e in epochs:
        b = by_epoch[e]
        rp = _median(b["rp"])
        qpc = _median(b["qpc"])
        fps = _median(b["fps"])
        if rp is not None:
            rp_by_epoch[e] = rp / 100.0
        rp_s = f"{rp / 100:.2f} vbl" if rp is not None else "n/a"
        qpc_s = f"{qpc / 1000:.1f} ms" if qpc is not None else "n/a"
        fps_s = f"{fps:.1f}" if fps is not None else "n/a"
        lbl = label.get(e, "") if isinstance(e, int) else ""
        print(
            f"  epoch {e} {lbl:16} settled_n={len(b['rp']):<4} "
            f"refresh/present={rp_s:10} frame={qpc_s:9} fps~{fps_s}"
        )

    if 0 in rp_by_epoch and 1 in rp_by_epoch:
        delta = rp_by_epoch[1] - rp_by_epoch[0]
        verdict = "PARITY" if abs(delta) <= NOISE_VBLANK else "DIP (reload heavier)"
        print(
            f"\n  AC-2 within-run delta (load2 - load1) = {delta:+.2f} vblank/present "
            f"-> {verdict}  (noise threshold +/-{NOISE_VBLANK})"
        )
        n2 = len(by_epoch[1]["rp"])
        if n2 < 20:
            print(f"  CAUTION: load2 settled_n={n2} is thin; want >=20 settled samples for a robust verdict.")
    else:
        print("\n  cannot compute AC-2 delta: need both load1 (epoch 0) and load2 (epoch 1) settled windows.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
