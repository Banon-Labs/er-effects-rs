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


def main() -> int:
    if len(sys.argv) < 2:
        print(__doc__)
        return 2
    rows, path = load(sys.argv[1])
    print(f"# {path}  ({len(rows)} phases)")
    if not rows:
        print("no phase telemetry -- did the harness write er-input-harness-phases.jsonl? (harness DLL loaded, phase-split build?)")
        return 1

    hdr = f"{'idx':>3} {'phase':<22} {'outcome':<9} {'ms':>7} {'frames':>7} {'state':>6} {'a40':>4} {'load_fsm':>8} {'world':>5}"
    print(hdr)
    print("-" * len(hdr))
    total_ms = 0
    for r in rows:
        dur_ms = r.get("duration_ms", -1)
        if isinstance(dur_ms, (int, float)) and dur_ms >= 0:
            total_ms += dur_ms
        outcome = r.get("outcome", "?")
        mark = "" if outcome == "advanced" else "  <-- DERAILED"
        print(
            f"{r.get('idx','?'):>3} {str(r.get('phase','?')):<22} {outcome:<9} "
            f"{dur_ms:>7} {r.get('duration_frames','?'):>7} {r.get('title_state','?'):>6} "
            f"{r.get('a40','?'):>4} {r.get('load_fsm','?'):>8} {r.get('world_sim','?'):>5}{mark}"
        )
    print("-" * len(hdr))
    print(f"total driven time: {total_ms} ms ({total_ms/1000:.1f}s) across {len(rows)} phases")
    derailed = [r for r in rows if r.get("outcome") != "advanced"]
    if derailed:
        print(f"DERAILED phases: {', '.join(str(r.get('phase')) for r in derailed)}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
