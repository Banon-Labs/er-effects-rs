#!/usr/bin/env python3
"""Per-epoch load trajectory analyzer for samechar-3x runs.

Reads telemetry-timeseries.jsonl and, for each load epoch
(system_quit_continue_confirm_fresh_deser_count), prints the timing of the
game-global load milestones and the FPS trajectory split into the
still-loading window vs the settled (now_loading cleared) window.

Purpose (2026-07-21): the decisive two-agent trace showed load1 needed ~7.6s
post-mms18 to reach can_move, while the FPS-regression teardown sampled load2's
fps only ~5s after render_group -> it measured load2's LOADING-cap fps and tore
the run down before load2 settled. This tool makes "did load2 settle + become
movable, and what is its SETTLED fps" answerable directly, per bd
fps-test-after-load2-finished-settled-not-during-load.

Usage: python3 scripts/analyze-samechar-epochs.py <artifact-dir-or-jsonl>
"""
from __future__ import annotations

import json
import os
import statistics
import sys


def as_bool(v) -> bool:
    return bool(v) and v not in (0, "0", "false", "False", None)


def as_int(v, d=0) -> int:
    try:
        return int(v)
    except (TypeError, ValueError):
        return d


def as_float(v, d=0.0) -> float:
    try:
        return float(v)
    except (TypeError, ValueError):
        return d


def load_rows(path: str):
    if os.path.isdir(path):
        path = os.path.join(path, "telemetry-timeseries.jsonl")
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


def fmt_t(ms) -> str:
    return "-" if ms is None else f"{ms / 1000.0:.1f}s"


def main() -> int:
    if len(sys.argv) < 2:
        print(__doc__)
        return 2
    rows, path = load_rows(sys.argv[1])
    print(f"# {path}  ({len(rows)} samples)")
    if not rows:
        return 1

    epochs: dict[int, list] = {}
    for r in rows:
        ep = as_int(r.get("system_quit_continue_confirm_fresh_deser_count"), 0)
        epochs.setdefault(ep, []).append(r)

    for ep in sorted(epochs):
        samples = epochs[ep]
        t0 = as_int(samples[0].get("t_ms"))
        t1 = as_int(samples[-1].get("t_ms"))

        def first_when(pred):
            for s in samples:
                if pred(s):
                    return as_int(s.get("t_ms"))
            return None

        t_present = first_when(lambda s: as_bool(s.get("oracle_player_present")))
        t_rgroup = first_when(lambda s: as_bool(s.get("oracle_chr_render_group_enabled")))
        t_nl_clear = first_when(
            lambda s: as_bool(s.get("oracle_player_present"))
            and as_int(s.get("oracle_now_loading"), 1) == 0
        )
        t_canmove = first_when(lambda s: as_bool(s.get("oracle_can_move")))
        t_mms18 = first_when(lambda s: as_int(s.get("oracle_stepfinish_mms_state"), -1) == 18)

        loading_fps = [
            as_float(s.get("oracle_fps"))
            for s in samples
            if as_float(s.get("oracle_fps")) > 0
            and as_bool(s.get("oracle_player_present"))
            and as_int(s.get("oracle_now_loading"), 1) != 0
        ]
        settled_fps = [
            as_float(s.get("oracle_fps"))
            for s in samples
            if as_float(s.get("oracle_fps")) > 0
            and as_int(s.get("oracle_now_loading"), 1) == 0
        ]
        # The GOAL's exact metric: fps in the PLAYABLE + MOVING window (can_move latched = the char is
        # movable and the harness is driving forward). This is the window to compare across loads.
        move_fps = [
            as_float(s.get("oracle_fps"))
            for s in samples
            if as_float(s.get("oracle_fps")) > 0 and as_bool(s.get("oracle_can_move"))
        ]
        did_moves = [as_int(s.get("oracle_did_move_frames"), 0) for s in samples]
        names = {s.get("oracle_char_name") for s in samples if s.get("oracle_char_name")}

        ramp = None
        if t_canmove is not None and t_mms18 is not None:
            ramp = (t_canmove - t_mms18) / 1000.0

        print(f"\n## epoch {ep}  (load{ep + 1})  window {fmt_t(t0)}..{fmt_t(t1)}  n={len(samples)}")
        print(f"   char_name        : {sorted(n for n in names if n)}")
        print(f"   present @        : {fmt_t(t_present)}")
        print(f"   mms18 @          : {fmt_t(t_mms18)}   (stale-owner marker, not authoritative)")
        print(f"   render_group @   : {fmt_t(t_rgroup)}")
        print(f"   now_loading clr @: {fmt_t(t_nl_clear)}   <-- load COMPLETE (settled-fps gate)")
        print(f"   can_move @       : {fmt_t(t_canmove)}   ramp post-mms18={ramp if ramp is None else f'{ramp:.1f}s'}")
        print(f"   did_move frames  : max={max(did_moves) if did_moves else 0} last={did_moves[-1] if did_moves else 0}")
        if loading_fps:
            print(f"   fps LOADING      : mean={statistics.mean(loading_fps):.0f} min={min(loading_fps):.0f} max={max(loading_fps):.0f} n={len(loading_fps)}")
        else:
            print("   fps LOADING      : (no loading-window samples)")
        if settled_fps:
            print(f"   fps SETTLED      : mean={statistics.mean(settled_fps):.0f} min={min(settled_fps):.0f} max={max(settled_fps):.0f} n={len(settled_fps)}")
        else:
            print("   fps SETTLED      : (NONE -- now_loading never cleared this epoch)")
        if move_fps:
            print(f"   fps PLAYABLE+MOVE: mean={statistics.mean(move_fps):.0f} min={min(move_fps):.0f} max={max(move_fps):.0f} n={len(move_fps)}   <-- goal metric")
        else:
            print("   fps PLAYABLE+MOVE: (NONE -- can_move never latched this epoch)")
        last = samples[-1]
        print(
            "   switch semaphores: "
            f"arm={as_int(last.get('oracle_switch_arm_count'), -1)} "
            f"deferred={as_int(last.get('oracle_switch_deferred_count'), -1)} "
            f"reload_phase={as_int(last.get('oracle_switch_reload_phase'), -1)} "
            f"committed={as_int(last.get('oracle_switch_reload_committed'), -1)} "
            f"player_present={as_int(last.get('oracle_switch_player_present'), -1)} "
            f"menu_job={as_int(last.get('oracle_switch_menu_job_present'), -1)}"
        )
        flip_spf = [
            as_float(s.get("oracle_flip_fixed_spf"))
            for s in samples
            if as_bool(s.get("oracle_can_move")) and as_float(s.get("oracle_flip_fixed_spf")) > 0
        ]
        flip_modes = sorted(
            {as_int(s.get("oracle_flip_mode_current"), -1) for s in samples if as_bool(s.get("oracle_can_move"))}
        )
        if flip_spf:
            spf = statistics.mean(flip_spf)
            print(
                f"   flip (playable)  : fixed_spf={spf:.4f} (~{1 / spf:.0f}fps cap) mode_current={flip_modes} "
                f"use_dyn_lock={as_int(last.get('oracle_flip_use_dynamic_lock'), -1)} "
                f"dyn_lock={last.get('oracle_flip_dynamic_fps_lock')}   <-- 0.05=20fps CAP, 0.0167=60"
            )
        else:
            print("   flip (playable)  : (no can_move samples with flip data -- old DLL?)")

    # settled-vs-settled cross-epoch fps comparison (the real regression question)
    print("\n## settled fps comparison (settled-vs-settled, the real regression check)")
    base = None
    for ep in sorted(epochs):
        sf = [
            as_float(s.get("oracle_fps"))
            for s in epochs[ep]
            if as_float(s.get("oracle_fps")) > 0
            and as_int(s.get("oracle_now_loading"), 1) == 0
        ]
        if not sf:
            print(f"   load{ep + 1}: no settled samples")
            continue
        m = statistics.mean(sf)
        if ep == 0:
            base = m
            print(f"   load1 (baseline): {m:.0f}fps  n={len(sf)}")
        else:
            ratio = f"{100 * m / base:.0f}% of load1" if base else "n/a"
            print(f"   load{ep + 1}: {m:.0f}fps  n={len(sf)}  ({ratio})")

    # GOAL check: fps in the PLAYABLE+MOVING window, compared across loads (no significant difference).
    print("\n## playable+moving fps comparison (GOAL: no significant difference across loads)")
    move_means = {}
    for ep in sorted(epochs):
        mf = [
            as_float(s.get("oracle_fps"))
            for s in epochs[ep]
            if as_float(s.get("oracle_fps")) > 0 and as_bool(s.get("oracle_can_move"))
        ]
        if mf:
            move_means[ep] = statistics.mean(mf)
            print(f"   load{ep + 1}: {move_means[ep]:.0f}fps  n={len(mf)}")
        else:
            print(f"   load{ep + 1}: no playable+moving samples (can_move never latched)")
    if len(move_means) >= 2:
        lo, hi = min(move_means.values()), max(move_means.values())
        spread = 100 * (hi - lo) / hi if hi > 0 else 0.0
        # "significant" placeholder threshold: >15% spread across loads = a real difference to explain.
        verdict = "PARITY" if spread <= 15.0 else "DIFFERENCE"
        print(f"   -> spread {spread:.0f}% across {len(move_means)} loads: {verdict} (<=15% = parity)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
