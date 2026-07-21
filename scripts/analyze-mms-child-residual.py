#!/usr/bin/env python3
"""Diff the MoveMapStep child (mms+0x108) scheduling state between load1 and load2 at mms=18.

The FD4 scheduler ticks load1's MoveMapStep child ~145x (field25 walks 0->9 -> mms19) but stops
ticking load2's after ~6 (field25 stuck 0, mms stuck 18). The product now publishes the child ptr +
header window (oracle_mms_child_ptr / h00..h30) every frame. This tool, given an observe timeseries
that has both load1 (deser=0) and load2 (deser=1), collects each epoch's child header WHILE mms=18 and
reports which field DIFFERS between the (ticking) load1 child and the (dropped) load2 child -- the
residual state the in-world teardown leaves that a fresh boot doesn't. Read-only.

Usage: python3 scripts/analyze-mms-child-residual.py <telemetry-timeseries.jsonl>
"""
from __future__ import annotations

import json
import sys
from collections import OrderedDict
from pathlib import Path

CHILD_FIELDS = [
    "oracle_mms_child_ez08_step",
    "oracle_mms_child_ez10",
    "oracle_mms_child_ez18",
    "oracle_mms_child_ez20",
    "oracle_mms_child_ez28",
    "oracle_mms_child_step10",
    "oracle_mms_child_step18",
    "oracle_mms_child_step40",
    "oracle_mms_child_step48",
]


def load(path: Path) -> list[dict]:
    rows = []
    for line in path.read_text(encoding="utf-8", errors="replace").splitlines():
        line = line.strip()
        if line:
            try:
                rows.append(json.loads(line))
            except json.JSONDecodeError:
                pass
    return sorted(rows, key=lambda r: r.get("t_ms", 0))


def epoch_child_at_mms18(rows: list[dict], epoch: int) -> "OrderedDict[str, list]":
    """Distinct value sequence of each child field, over the rows where this epoch sits at mms=18."""
    out: OrderedDict[str, list] = OrderedDict((f, []) for f in CHILD_FIELDS)
    for r in rows:
        if r.get("system_quit_continue_confirm_fresh_deser_count") != epoch:
            continue
        if r.get("oracle_stepfinish_mms_state") != 18:
            continue
        for f in CHILD_FIELDS:
            v = r.get(f)
            if not out[f] or out[f][-1] != v:
                out[f].append(v)
    return out


def main() -> int:
    if len(sys.argv) < 2:
        print(__doc__)
        return 2
    rows = load(Path(sys.argv[1]))
    if not rows:
        print("no rows")
        return 1
    if not any("oracle_mms_child_ez08_step" in r for r in rows):
        print("timeseries has NO corrected oracle_mms_child_* fields (old DLL build) -- rebuild + re-run.")
        return 1
    ep_set: set[int] = set()
    for r in rows:
        v = r.get("system_quit_continue_confirm_fresh_deser_count")
        if isinstance(v, int):
            ep_set.add(v)
    epochs = sorted(ep_set)
    print(f"epochs present: {epochs}")
    per = {e: epoch_child_at_mms18(rows, e) for e in epochs}
    for e in epochs:
        n = max((len(v) for v in per[e].values()), default=0)
        print(f"\n== epoch {e} (mms=18 window, {n} distinct-value steps max) ==")
        for f in CHILD_FIELDS:
            vals = per[e][f]
            print(f"  {f}: {vals[:8]}{' ...' if len(vals) > 8 else ''}")
    # DIFF load1 (0) vs load2 (1): fields whose STABLE mms=18 value differs.
    if 0 in per and 1 in per:
        print("\n== load1(0) vs load2(1) at mms=18 -- residual-state candidates ==")

        def stable(seq):
            return seq[-1] if seq else None

        for f in CHILD_FIELDS:
            a, b = stable(per[0][f]), stable(per[1][f])
            changed_l2 = len(per[1][f]) > 1
            note = ""
            if a != b:
                note = "  <-- DIFFERS load1 vs load2"
            if changed_l2:
                note += "  (load2 value CHANGED during its mms=18 stall)"
            print(f"  {f}: load1={a} load2={b}{note}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
