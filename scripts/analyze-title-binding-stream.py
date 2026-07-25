#!/usr/bin/env python3
"""Summarize / diff the decoupled Phase-A oracle streams for the warm-title-rebuild A/B.

Two independent, per-oracle-marker-gated read streams (bd decoupled-diagnostics-architecture-buildplan-
2026-07-24), emitted by er-telemetry-dll when the matching er-oracle-*.on marker is present:

  er-oracle-title-binding.jsonl  -- the crux: does the (armed) warm switch title bind dialog+0xb78?
      fields: epoch, play_time_ms, dialog_ptr, proxy_bound, proxy_handle_nonnull, tfc_14c, tfc_18c,
              menujob_ptr
  er-oracle-stream-overlap.jsonl -- the fix's success metric: is CSWorldGeomMan streaming still running
      while the player is movable? fields: epoch, play_time_ms, streaming_active, player_movable,
      overlap, overlap_run, flip_task_delta

The A/B is product {armed, disarmed} x an IDENTICAL diagnostic set, so any delta is the product feature:
- title-binding: the armed arm is expected to FAIL to bind (proxy_handle_nonnull stays false) -- that is
  the deadlock this experiment localizes. proxy_bound True in disarmed but False in armed pins the hold.
- stream-overlap: overlap==True while player_movable==True is the dip mechanism; the fix should drive it
  toward 0 (streaming settles before movable).

Usage:
  python3 scripts/analyze-title-binding-stream.py <dir-or-glob-root>            # summarize one run
  python3 scripts/analyze-title-binding-stream.py --compare <armed_dir> <disarmed_dir>

A "dir" may be a run artifact dir OR the game dir (default game dir if a bare oracle file is not found).
"""
from __future__ import annotations

import glob
import json
import os
import statistics
import sys

DEFAULT_GAME_DIR = os.environ.get(
    "GAME_DIR", "/mnt/c/SteamLibrary/steamapps/common/ELDEN RING/Game"
)
TITLE_FILE = "er-oracle-title-binding.jsonl"
STREAM_FILE = "er-oracle-stream-overlap.jsonl"


def _find(d: str, name: str) -> str | None:
    for cand in (os.path.join(d, name), os.path.join(DEFAULT_GAME_DIR, name)):
        if os.path.exists(cand):
            return cand
    hits = glob.glob(os.path.join(d, "**", name), recursive=True)
    return hits[0] if hits else None


def _rows(path: str | None) -> list[dict]:
    if not path or not os.path.exists(path):
        return []
    out = []
    for line in open(path, encoding="utf-8", errors="replace"):
        line = line.strip()
        if not line:
            continue
        try:
            out.append(json.loads(line))
        except json.JSONDecodeError:
            continue
    return out


def _truthy(v) -> bool:
    return v is True or v == 1 or v == "true"


def _by_epoch(rows: list[dict]) -> dict:
    d: dict = {}
    for r in rows:
        d.setdefault(r.get("epoch"), []).append(r)
    return d


def _frac(rows: list[dict], field: str) -> str:
    vals = [r[field] for r in rows if field in r]
    if not vals:
        return "n/a"
    t = sum(1 for v in vals if _truthy(v))
    return f"{t}/{len(vals)} ({100 * t / len(vals):.0f}%)"


def summarize(d: str, label: str = "") -> dict:
    trows = _rows(_find(d, TITLE_FILE))
    srows = _rows(_find(d, STREAM_FILE))
    print(f"\n===== {label or d} =====")
    print(f"title-binding rows: {len(trows)}   stream-overlap rows: {len(srows)}")
    summary: dict = {"title": {}, "stream": {}}

    if trows:
        print("-- title-binding (dialog+0xb78) by epoch --")
        for e in sorted(_by_epoch(trows), key=lambda x: (x is None, x)):
            rows = _by_epoch(trows)[e]
            bound = _frac(rows, "proxy_bound")
            handle = _frac(rows, "proxy_handle_nonnull")
            print(f"   epoch {e}: proxy_bound {bound}   handle_nonnull {handle}   n={len(rows)}")
            summary["title"][e] = {"proxy_bound": bound, "handle_nonnull": handle}
    if srows:
        print("-- stream-overlap (streaming-vs-movable) by epoch --")
        for e in sorted(_by_epoch(srows), key=lambda x: (x is None, x)):
            rows = _by_epoch(srows)[e]
            movable = [r for r in rows if _truthy(r.get("player_movable"))]
            ov = _frac(movable, "overlap") if movable else "n/a (never movable)"
            runs = [int(r["overlap_run"]) for r in rows if isinstance(r.get("overlap_run"), (int, float))]
            fd = [float(r["flip_task_delta"]) for r in rows if isinstance(r.get("flip_task_delta"), (int, float)) and r["flip_task_delta"] > 0]
            fps = f"{1.0 / statistics.median(fd):.1f}" if fd else "n/a"
            print(
                f"   epoch {e}: overlap|movable {ov}   max_overlap_run {max(runs) if runs else 'n/a'}   "
                f"median_fps {fps}   n={len(rows)}"
            )
            summary["stream"][e] = {"overlap_while_movable": ov, "max_run": max(runs) if runs else None, "fps": fps}
    if not trows and not srows:
        print("   (no oracle streams found -- were the er-oracle-*.on markers present for this run?)")
    return summary


def main() -> int:
    args = sys.argv[1:]
    if len(args) >= 3 and args[0] == "--compare":
        summarize(args[1], "ARMED")
        summarize(args[2], "DISARMED")
        print(
            "\n== A/B read: title-binding proxy_handle_nonnull True(disarmed) vs False(armed) pins the "
            "hold that blocks the armed native Continue; stream-overlap overlap|movable is the dip "
            "mechanism (fix drives it toward 0)."
        )
        return 0
    if len(args) == 1:
        summarize(args[0])
        return 0
    print(__doc__)
    return 2


if __name__ == "__main__":
    raise SystemExit(main())
