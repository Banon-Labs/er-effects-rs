#!/usr/bin/env python3
"""Capture a load SEQUENCE (load1 boot -> load2 -> load3 ...) and DIFF the per-load settled semaphores.

Purpose (user 2026-07-18): the freeze is DETERMINISTIC on the SECOND load and RECOVERS on the third
(load1 renders, load2 freezes, load3 renders). This is a stale-state/parity bug: load1 leaves some
state stale that breaks load2's render handoff; load2's teardown clears it so load3 is fine. The
render-gated proof monitor STOPS at the frozen load2, so it can never reach load3. This driver instead
triggers each subsequent load on a TIMER (regardless of render success), snapshots the settled RAM
semaphores at each load, and DIFFs consecutive loads -- the field(s) that differ between the frozen
load2 and the good load3 name the stale element (the bug).

It does NOT gate on render success and does NOT judge pass/fail -- it is a pure diagnostic capture.
Zero simulated input: switches are triggered by writing the next (file,)slot to the DLL control files,
exactly like multi-load-proof-monitor.py's programmatic drive.

Usage:
  capture-load-sequence.py --artifact-dir GAME_DIR --switch-slot-file F [--switch-file-override F]
      --slots "1,6" [--boot-timeout 120] [--per-load 55] [--report OUT.md]
  # slots = the reload targets AFTER boot; e.g. "1,6" does boot -> load2(slot1) -> load3(slot6).
"""
from __future__ import annotations

import argparse
import json
import threading
import time
from pathlib import Path

# Semaphore fields worth diffing across loads (render handoff + the mms18 finalize gate + stale-flag
# suspects). Kept explicit so the diff is readable and stable.
DIFF_KEYS = [
    "oracle_char_name", "oracle_char_level",
    "oracle_player_render_ready", "oracle_chr_draw_group_enabled",
    "oracle_chr_render_group_enabled", "oracle_chr_enable_render", "oracle_chr_onscreen",
    "oracle_now_loading", "oracle_fake_loading_any_visible",
    "oracle_stepfinish_request_code", "oracle_stepfinish_warmup",
    "oracle_stepfinish_testnet_stepper_present", "oracle_stepfinish_mms_state",
    "oracle_csremo_present", "oracle_csremo_remoman_present", "oracle_csremo_remo_pending",
    "oracle_own_load_wbr_max_phase", "oracle_own_load_wbr_any_gate_set",
    "oracle_saved_map_c30", "oracle_play_time_ms",
    "system_quit_quickload_phase", "system_quit_quickload_return_title_request_count",
]
_WAIT = threading.Event()


def read_tel(p: Path):
    try:
        return json.loads(p.read_text())
    except (OSError, json.JSONDecodeError):
        return None


def player_in_world(t: dict) -> bool:
    name = t.get("oracle_char_name") or ""
    return (t.get("oracle_player_present") is True or t.get("player_available") is True) \
        and bool(name) and not name.startswith("翿") \
        and (t.get("oracle_char_level") or 0) > 0


def wait_settled(telem: Path, cap_s: float, poll: float) -> dict:
    """Wait until the load 'settles': mms_state + render_ready stop changing for ~8s, or cap_s hits.
    Returns the last telemetry snapshot. Always returns even on a frozen (never-rendering) load."""
    start = time.time()
    last_key, stable_since = None, start
    snap = {}
    while time.time() - start < cap_s:
        t = read_tel(telem)
        if t is not None:
            snap = t
            key = (t.get("oracle_stepfinish_mms_state"), t.get("oracle_player_render_ready"),
                   t.get("oracle_chr_draw_group_enabled"), t.get("oracle_char_name"))
            now = time.time()
            if key != last_key:
                last_key, stable_since = key, now
            elif now - stable_since >= 8.0 and t.get("oracle_char_name"):
                break
        _WAIT.wait(poll)
    return snap


def snap_fields(t: dict) -> dict:
    return {k: t.get(k) for k in DIFF_KEYS}


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--artifact-dir", required=True, type=Path)
    ap.add_argument("--switch-slot-file", required=True, type=Path)
    ap.add_argument("--switch-file-override", type=Path, default=None)
    ap.add_argument("--slots", required=True, help="comma-separated reload slots AFTER boot, e.g. '1,6'")
    ap.add_argument("--switch-files", default="", help="optional comma-separated Windows save paths per slot (cross-file)")
    ap.add_argument("--boot-timeout", type=float, default=120.0)
    ap.add_argument("--per-load", type=float, default=55.0)
    ap.add_argument("--poll", type=float, default=2.0)
    ap.add_argument("--report", type=Path, default=None)
    args = ap.parse_args()

    telem = args.artifact_dir / "er-effects-telemetry.json"
    slots = [int(s) for s in args.slots.replace(",", " ").split()]
    files = [s.strip() for s in args.switch_files.split(",")] if args.switch_files.strip() else []
    snapshots = []  # (label, fields)

    # LOAD 1 (boot): wait for the boot autoload to reach in-world, then settle.
    t0 = time.time()
    while time.time() - t0 < args.boot_timeout:
        t = read_tel(telem)
        if t is not None and player_in_world(t):
            break
        _WAIT.wait(args.poll)
    snapshots.append(("load1-boot", snap_fields(wait_settled(telem, 20, args.poll))))
    print(f"load1-boot settled: {snapshots[-1][1].get('oracle_char_name')} "
          f"render_ready={snapshots[-1][1].get('oracle_player_render_ready')}", flush=True)

    # LOAD 2..N: trigger each on a timer regardless of render success, then settle + snapshot.
    for i, slot in enumerate(slots):
        try:
            if args.switch_file_override is not None and i < len(files) and files[i]:
                args.switch_file_override.write_text(files[i] + "\n")
            args.switch_slot_file.write_text(f"{slot}\n")
        except OSError as e:
            print(f"WARN: could not write switch control file: {e}", flush=True)
        label = f"load{i + 2}-slot{slot}"
        snap = snap_fields(wait_settled(telem, args.per_load, args.poll))
        snapshots.append((label, snap))
        print(f"{label} settled: {snap.get('oracle_char_name')} "
              f"render_ready={snap.get('oracle_player_render_ready')} "
              f"mms={snap.get('oracle_stepfinish_mms_state')} "
              f"draw_group={snap.get('oracle_chr_draw_group_enabled')}", flush=True)

    # DIFF consecutive loads (the frozen-vs-good comparison names the stale element).
    lines = ["# Load-sequence semaphore capture + diff", ""]
    lines.append("| field | " + " | ".join(lbl for lbl, _ in snapshots) + " |")
    lines.append("|---|" + "|".join(["---"] * len(snapshots)) + "|")
    for k in DIFF_KEYS:
        vals = [str(f.get(k)) for _, f in snapshots]
        mark = " **<-- differs**" if len(set(vals)) > 1 else ""
        lines.append(f"| {k} | " + " | ".join(vals) + f" |{mark}")
    lines.append("")
    lines.append("## Consecutive diffs (the field(s) that flip frozen->good = the stale element)")
    for a in range(len(snapshots) - 1):
        (la, fa), (lb, fb) = snapshots[a], snapshots[a + 1]
        diffs = [f"{k}: {fa.get(k)} -> {fb.get(k)}" for k in DIFF_KEYS if fa.get(k) != fb.get(k)]
        lines.append(f"\n### {la} -> {lb}")
        lines.extend(f"- {d}" for d in diffs) if diffs else lines.append("- (no differences)")
    report = "\n".join(lines)
    print("\n" + report, flush=True)
    if args.report:
        args.report.write_text(report)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
