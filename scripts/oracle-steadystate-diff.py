#!/usr/bin/env python3
"""Oracle STEADY-STATE semaphore parity diff (Milestone-1, acceptance strengthened 2026-07-22).

The trajectory comparator (oracle-compare.py) walks ordered DISCRETE transitions and catches a
load that takes the wrong path / wrong order / wrong timing. It does NOT catch a divergence that
only appears AFTER the character is movable and every discrete step already matched -- the classic
example being the 20 fps reload (fps is a continuous field, not a transition) and its co-divergent
`oracle_chr_draw_group_enabled == False`. The strengthened acceptance judges parity over BOTH the
load trajectory AND a *sustained post-readiness steady-state window*. This tool is that second half.

It takes a BASELINE window (vanilla continue/reload -- the ground truth) and a CANDIDATE window (the
mod's switched reload), slices each to the sustained post-readiness window (oracle_can_move truthy,
first `--settle` frames dropped so asset-streaming transients don't mask the persistent component),
NORMALIZES the inherently-nondeterministic fields (§3b of the goal: heap/RNG/wall-clock/frame-index/
monotonic counters), and emits an ORDERED divergence list -- every scalar/categorical semaphore whose
steady-state value differs, most structurally-severe first. Pass == empty diff (exact after
normalization, zero tolerance).

Two input modes:
  two files : --baseline vanilla.jsonl --candidate mod.jsonl
  one file  : <file> --baseline-epoch N --candidate-epoch M   (slice one run by oracle_current_load_epoch)

Exit code: 0 if the steady-state diff is empty, 1 if any semaphore diverged, 2 on insufficient data.
"""
from __future__ import annotations

import argparse
import json
import statistics as st
import sys
from collections import Counter
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import oracle_common as oc  # noqa: E402

# --- Field normalization classes (§3b: normalize the inherently-nondeterministic, then exact) -------
#
# DROP: fields that legitimately differ run-to-run or are phase keys / monotonic counters / raw
# pointer dumps. They carry no steady-state parity meaning once normalized away.
DROP_FIELDS = {
    # phase / epoch keys (which load, not a semaphore value)
    "oracle_current_load_epoch",
    "oracle_boot_view_epoch_live",
    "system_quit_continue_confirm_fresh_deser_count",
    "sq_repro_state",
    "sq_repro_switch_index",
    # world position / physics -- differs by exact spawn micro-jitter (movement proof, not parity)
    "oracle_havok_pos",
    "oracle_did_move_frames",
    "oracle_move_probe_moved_frames",
    "oracle_supplied_movement_input_frames",
    "oracle_harness_move_verdict",
    # wall-clock-ish / play-time (relativized elsewhere; not a steady-state value)
    "oracle_play_time_ms",
    "oracle_play_time_live",
    "oracle_play_time_advanced_ms",
    # monotonic hit/event counters -- grow unbounded with window length, not a state value
    "oracle_gx_cmdqueue_reserves",  # cumulative reserve counter -- grows with window length
    "oracle_loading_bar_update_hits",
    "oracle_loading_bar_final_hits",
    "oracle_loading_screen_close_sent_hits",
    "oracle_rawinput_hook_calls",
    "oracle_rawinput_key_events",
    "oracle_rawinput_mouse_button_events",
    "oracle_rawinput_mouse_move_events",
    "oracle_rawinput_blocked_unfocused_events",
    "oracle_switch_arm_count",
    "oracle_switch_deferred_count",
    "oracle_switch_teardown_count",
    "oracle_switch_reload_drain_waits",
    # mod-internal switch FSM bookkeeping -- no vanilla counterpart, describes the mod's own switch
    # state machine, not a GAME-state semaphore. Parity is about game state, so these are scaffolding.
    "oracle_switch_last_slot",
    "oracle_switch_reload_phase",
    "oracle_switch_reload_committed",
    "oracle_switch_player_present",
    "oracle_switch_menu_job_present",
    "oracle_switch_stable_frames",
    "oracle_switch_slot_control_primed",
    "oracle_switch_slot_control_mtime",  # source-file mtime (wall-clock)
    "system_quit_continue_confirm_allow_count",
    "system_quit_continue_confirm_fresh_deser_count",
    "system_quit_continue_confirm_fresh_deser_done",
    "system_quit_profile_load_activate_count",
    # frame indices / progress counters (index, not state)
    "oracle_loading_bar_current_frame",
    "oracle_loading_bar_max_frame",
    "oracle_loading_bar_progress_permille",
    "oracle_flip_last_frame_time",
    # raw pointer / opaque-dump fields (heap addresses -- would need module-relative canonicalization;
    # they carry no independent parity signal beyond the present/booleans that summarize them)
    "oracle_gx_cmdqueue_top_producers",
    "oracle_loading_screen_last_data",
    "oracle_loading_screen_last_this",
    "oracle_system_step_state",  # opaque rotating pointer-ish value (label carries the meaning)
    "t_ms",
}

# NUMERIC: continuous fields compared on a canonicalized median, rounded to `unit` so sub-unit jitter
# is not a divergence but a real regime change (20 vs 60 fps) is. Unit chosen per field's meaning.
NUMERIC_UNITS = {
    "oracle_fps": 1.0,            # whole fps
    "oracle_flip_calc_fps": 1.0,
    "oracle_min_fps": 1.0,
    "oracle_frame_ms": 1.0,       # 1 ms
    "oracle_flip_task_delta": 0.005,   # seconds (5 ms)
    "oracle_flip_fixed_spf": 0.001,
    "oracle_game_task_us": 250.0,      # 0.25 ms buckets
    "oracle_build_driver_us": 250.0,
    "oracle_composite_us": 250.0,
    "oracle_present_call_us": 250.0,
    "oracle_present_qpc_delta_us": 8000.0,   # ~half a vblank
    "oracle_present_refresh_per_present_x100": 50.0,  # half a refresh
    "oracle_gx_cmdqueue_max_fill": 1.0,
    "oracle_gx_cmdqueue_reserves": 1.0,
}


def _f(v):
    try:
        return float(v)
    except (TypeError, ValueError):
        return None


def _truthy(v) -> bool:
    return v in (1, True, "1", "true", "True")


import re

_PTR_RE = re.compile(r"^0x[0-9a-fA-F]+$")


def _canon(v):
    """Canonical categorical value with §3b normalization.

    Heap-pointer-valued semaphores (raw 0x... addresses) are inherently nondeterministic run-to-run;
    offline we cannot module-relativize them, so we normalize to null/non-null PRESENCE, which is the
    honest, deterministic parity signal (a pointer being set vs 0x0 is meaningful; its exact address is
    not). Everything else compares order-insensitively.
    """
    if isinstance(v, bool):
        return v
    if isinstance(v, str) and _PTR_RE.match(v):
        return "<ptr:null>" if v in ("0x0", "0x00") else "<ptr:set>"
    return json.dumps(v, sort_keys=True) if isinstance(v, (list, dict)) else v


def steady_window(
    rows: list[dict], epoch: int | None, settle: int, window_field: str = "oracle_can_move"
) -> list[dict]:
    """Sustained post-readiness window: rows where `window_field` is truthy, first `settle` dropped.

    Default `oracle_can_move` = genuinely movable (the strictest readiness). A vanilla telemetry-only
    capture driven in BOOT mode holds the character in-world without movement injection, so use
    `oracle_player_present` there -- the char is rendered + cadence-populated even if can_move never
    latched. Comparing a vanilla present-window to a mod can_move-window is honest for the render-bound
    fps/cadence/GX semaphores (they are readiness-independent); the caller picks the field per side.
    """
    sel = rows
    if epoch is not None:
        def _ep(r):
            e = _f(r.get("oracle_current_load_epoch"))
            return int(e) if e is not None else None
        sel = [r for r in rows if _ep(r) == epoch]
    win = [r for r in sel if _truthy(r.get(window_field))]
    return win[settle:] if len(win) > settle else win


def summarize(window: list[dict]) -> dict:
    """Per-field steady-state summary: numeric -> median/rounded; categorical -> mode + stability."""
    if not window:
        return {}
    fields = set()
    for r in window:
        fields.update(k for k in r if k.startswith("oracle_") or k.startswith("system_") or k in ("sq_repro_state", "sq_repro_switch_index"))
    fields -= DROP_FIELDS
    out = {}
    for fld in fields:
        vals = [r[fld] for r in window if fld in r and r[fld] is not None]
        if not vals:
            continue
        if fld in NUMERIC_UNITS:
            nums = [x for x in (_f(v) for v in vals) if x is not None]
            if not nums:
                continue
            unit = NUMERIC_UNITS[fld]
            med = st.median(nums)
            out[fld] = {
                "kind": "numeric",
                "median": med,
                "canon": round(med / unit) * unit,
                "p5": sorted(nums)[max(0, int(len(nums) * 0.05))],
                "p95": sorted(nums)[min(len(nums) - 1, int(len(nums) * 0.95))],
                "unit": unit,
                "n": len(nums),
            }
        else:
            c = Counter(_canon(v) for v in vals)
            mode = c.most_common(1)[0][0]
            out[fld] = {
                "kind": "categorical",
                "mode": mode,
                "stable": len(c) == 1,
                "distinct": len(c),
                "n": len(vals),
            }
    return out


def diff(base: dict, cand: dict) -> list[dict]:
    findings = []
    for fld in sorted(set(base) | set(cand)):
        b, c = base.get(fld), cand.get(fld)
        if b is None or c is None:
            findings.append({
                "field": fld, "sev": 3, "kind": "presence",
                "detail": f"present in {'candidate' if b is None else 'baseline'} only",
                "base": b, "cand": c,
            })
            continue
        if b["kind"] != c["kind"]:
            findings.append({"field": fld, "sev": 3, "kind": "typechange",
                             "detail": f"{b['kind']} vs {c['kind']}", "base": b, "cand": c})
            continue
        if b["kind"] == "categorical":
            if b["mode"] != c["mode"]:
                findings.append({
                    "field": fld, "sev": 2, "kind": "categorical",
                    "detail": f"baseline={b['mode']!r}  candidate={c['mode']!r}",
                    "base": b, "cand": c,
                })
        else:  # numeric
            if b["canon"] != c["canon"]:
                bm, cm = b["median"], c["median"]
                rel = abs(cm - bm) / (abs(bm) + 1e-9)
                findings.append({
                    "field": fld, "sev": 1, "kind": "numeric",
                    "detail": f"baseline median={bm:.3f} (canon {b['canon']:g})  "
                              f"candidate median={cm:.3f} (canon {c['canon']:g})  Δ={cm - bm:+.3f}",
                    "rel": rel, "base": b, "cand": c,
                })
    # order: structural (presence/typechange) first, then categorical, then numeric by relative Δ desc
    findings.sort(key=lambda x: (-x["sev"], -x.get("rel", 0.0), x["field"]))
    return findings


def load(path: Path) -> list[dict]:
    return oc.load_rows(path)


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("timeseries", nargs="?", type=Path, help="single-file mode: one timeseries.jsonl")
    ap.add_argument("--baseline", type=Path, help="two-file mode: baseline (vanilla) timeseries.jsonl")
    ap.add_argument("--candidate", type=Path, help="two-file mode: candidate (mod) timeseries.jsonl")
    ap.add_argument("--baseline-epoch", type=int, default=None, help="single-file: baseline load epoch")
    ap.add_argument("--candidate-epoch", type=int, default=None, help="single-file: candidate load epoch")
    ap.add_argument("--baseline-window", default="oracle_can_move",
                    help="readiness field for the baseline window (e.g. oracle_player_present for a "
                         "boot-mode vanilla hold that never latches can_move)")
    ap.add_argument("--candidate-window", default="oracle_can_move",
                    help="readiness field for the candidate window")
    ap.add_argument("--settle", type=int, default=60, help="drop first N movable frames per window")
    ap.add_argument("--min-frames", type=int, default=20, help="minimum settled frames to compare")
    ap.add_argument("--json", action="store_true", help="emit findings as JSON")
    a = ap.parse_args()

    if a.baseline and a.candidate:
        brows, crows = load(a.baseline), load(a.candidate)
        bwin = steady_window(brows, a.baseline_epoch, a.settle, a.baseline_window)
        cwin = steady_window(crows, a.candidate_epoch, a.settle, a.candidate_window)
        bsrc = f"{a.baseline} epoch={a.baseline_epoch}"
        csrc = f"{a.candidate} epoch={a.candidate_epoch}"
    elif a.timeseries and a.baseline_epoch is not None and a.candidate_epoch is not None:
        rows = load(a.timeseries)
        bwin = steady_window(rows, a.baseline_epoch, a.settle, a.baseline_window)
        cwin = steady_window(rows, a.candidate_epoch, a.settle, a.candidate_window)
        bsrc = f"{a.timeseries} epoch={a.baseline_epoch}"
        csrc = f"{a.timeseries} epoch={a.candidate_epoch}"
    else:
        ap.error("provide --baseline+--candidate OR <file> --baseline-epoch N --candidate-epoch M")

    if len(bwin) < a.min_frames or len(cwin) < a.min_frames:
        print(f"INSUFFICIENT steady-state data: baseline={len(bwin)} candidate={len(cwin)} "
              f"settled frames (need >= {a.min_frames}). settle={a.settle}.")
        return 2

    bsum, csum = summarize(bwin), summarize(cwin)
    findings = diff(bsum, csum)

    if a.json:
        print(json.dumps({"baseline": bsrc, "candidate": csrc,
                          "baseline_frames": len(bwin), "candidate_frames": len(cwin),
                          "findings": findings}, indent=2, default=str))
        return 0 if not findings else 1

    print(f"STEADY-STATE PARITY DIFF")
    print(f"  baseline : {bsrc}  ({len(bwin)} settled frames)")
    print(f"  candidate: {csrc}  ({len(cwin)} settled frames)")
    print(f"  settle-drop={a.settle}  fields compared={len(set(bsum) | set(csum))}\n")
    if not findings:
        print("  ✓ EMPTY DIFF -- steady-state semaphores match exactly after normalization.")
        return 0
    print(f"  ✗ {len(findings)} DIVERGENCE(S) (most-severe first):\n")
    labels = {3: "STRUCT", 2: "CATEG ", 1: "NUMERIC"}
    for i, fnd in enumerate(findings, 1):
        print(f"  [{i:2d}] {labels[fnd['sev']]}  {fnd['field']}")
        print(f"        {fnd['detail']}")
    print("\n  => fix in the order above; re-diff until EMPTY (zero tolerance after normalization).")
    return 1


if __name__ == "__main__":
    raise SystemExit(main())
