#!/usr/bin/env python3
"""Phase-1 render-residency diff for the switch-reload fps investigation.

The AC-2 answer (bd AC-2-ANSWERED-native-reload-no-dip-mod-ownload-dips) is: the vanilla NATIVE reload
LIGHTENS (releases render resources) while the mod own_load_switch_reload_fire reload STAYS heavy. This
tool reads a run's per-frame telemetry-timeseries.jsonl, segments SETTLED (player_present) rows by load
epoch (fresh_deser_count / current_load_epoch), and reports the median of the read-side render-residency
oracles per epoch so you can see WHETHER the reload releases (residency drops load1 -> reload) or retains
(stays flat/higher).

Render-residency fields (added to the per-frame writer for Phase 1; passive reads of the game's own
structures):
  oracle_gxdc_output_count / _span_bytes / _capacity  -- GxDrawContext render-output vector size (the
      count of live render outputs; a reload that does NOT release leaves extra outputs resident)
  oracle_render_distview_mgr_ptr / oracle_render_mapitem_mgr_ptr -- the exact render managers the native
      _Common_Finalize teardown frees (nonzero = resident)
It also shows the existing GX cmdqueue fill/reserves for cross-reference.

Usage:
    python3 scripts/analyze-render-residency.py <artifact-dir-or-timeseries.jsonl>

Compare TWO runs (mod vs vanilla), or within one run compare epoch 0 (first load) vs epoch 1 (reload):
if the reload residency does NOT fall relative to the first load, that is the retained render state that
own_load skips releasing (the dip). See bd GOAL-REFRAME / STEP4.
"""
from __future__ import annotations

import json
import statistics
import sys
from pathlib import Path

EPOCH_KEYS = (
    "fresh_deser_count",
    "oracle_fresh_deser_count",
    "oracle_current_load_epoch",
    "oracle_switch_reload_committed",
)
PRESENT_KEYS = ("oracle_switch_player_present", "oracle_player_present")

# Numeric render-residency + cross-reference fields (median per epoch).
NUMERIC_FIELDS = (
    "oracle_gxdc_output_count",
    "oracle_gxdc_output_span_bytes",
    "oracle_gxdc_output_capacity",
    "oracle_gx_cmdqueue_max_fill",
    "oracle_gx_cmdqueue_reserves",
    "oracle_present_refresh_per_present_x100",
)
# Pointer fields: report resident-fraction (nonzero / n).
PTR_FIELDS = (
    "oracle_gxdc_ptr",
    "oracle_render_distview_mgr_ptr",
    "oracle_render_mapitem_mgr_ptr",
)

LABELS = {0: "load1(firstload)", 1: "load2(reload)", 2: "load3(reload2)"}


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


def _as_int(v):
    """Coerce a field that may be an int, a float, or a hex/dec string pointer to an int (or None)."""
    if isinstance(v, bool):
        return int(v)
    if isinstance(v, (int, float)):
        return int(v)
    if isinstance(v, str):
        s = v.strip()
        try:
            return int(s, 16) if s.lower().startswith("0x") else int(s)
        except ValueError:
            return None
    return None


def main() -> int:
    if len(sys.argv) != 2:
        print(__doc__)
        return 2
    arg = Path(sys.argv[1])
    ts = arg / "telemetry-timeseries.jsonl" if arg.is_dir() else arg
    if not ts.exists():
        print(f"ERROR: no timeseries at {ts}", file=sys.stderr)
        return 2

    by_epoch: dict[object, list[dict]] = {}
    present_fields: set[str] = set()
    for line in ts.read_text(encoding="utf-8", errors="replace").splitlines():
        line = line.strip()
        if not line:
            continue
        try:
            row = json.loads(line)
        except json.JSONDecodeError:
            continue
        present_fields.update(row.keys())
        if not _settled(row):
            continue
        e = _epoch(row)
        if e is None:
            continue
        by_epoch.setdefault(e, []).append(row)

    print(f"# {ts}")
    have_render = [f for f in (*NUMERIC_FIELDS, *PTR_FIELDS) if f in present_fields]
    if not any(f.startswith("oracle_gxdc") or "distview" in f or "mapitem" in f for f in have_render):
        print("WARNING: no render-residency fields (oracle_gxdc_*/distview/mapitem) present in this")
        print("         timeseries -- they were not wired into the per-frame writer for this run.")
    epochs = sorted(by_epoch.keys(), key=lambda x: (x is None, x))
    if not epochs:
        print("no settled (player_present) rows -- run never reached a movable in-world window")
        return 1

    for e in epochs:
        rows = by_epoch[e]
        lbl = LABELS.get(e, "") if isinstance(e, int) else ""
        print(f"\n== epoch {e} {lbl}  (settled_n={len(rows)}) ==")
        for f in NUMERIC_FIELDS:
            vals = [iv for iv in (_as_int(r.get(f)) for r in rows) if iv is not None and iv >= 0]
            if vals:
                print(f"  {f:44} median={statistics.median(vals):>14}  min={min(vals)} max={max(vals)}")
        for f in PTR_FIELDS:
            vals = [_as_int(r.get(f)) for r in rows]
            vals = [iv for iv in vals if iv is not None]
            if vals:
                resident = sum(1 for iv in vals if iv > 0x10000)
                print(f"  {f:44} resident={resident}/{len(vals)} frames")

    if 0 in by_epoch and 1 in by_epoch:
        print("\n== load1 -> reload residency delta (retained if it does NOT fall) ==")
        for f in ("oracle_gxdc_output_count", "oracle_gxdc_output_span_bytes"):
            a = [iv for iv in (_as_int(r.get(f)) for r in by_epoch[0]) if iv is not None and iv >= 0]
            b = [iv for iv in (_as_int(r.get(f)) for r in by_epoch[1]) if iv is not None and iv >= 0]
            if a and b:
                ma, mb = statistics.median(a), statistics.median(b)
                verdict = "RETAINED (reload >= load1)" if mb >= ma else "released (reload < load1)"
                print(f"  {f:44} load1={ma} reload={mb} -> {verdict}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
