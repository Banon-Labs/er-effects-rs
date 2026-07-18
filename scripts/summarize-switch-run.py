#!/usr/bin/env python3
"""Summarize a single multi-save-load run's DLL debug log: does a genuine switch complete or stall?

Reads an er-effects-autoload-debug.log slice (e.g. a run's my-run-debug.log) and reports the
switch-outcome story WITHOUT eyeballing: character-name transitions, the FIX-1 disarm events,
SWITCH-ORACLE class transitions, the MoveMapStep-state histogram + the final/most-common stuck
step, the world-res-wait / step-18 signatures, and any genuine crash/assert markers. This turns the
raw log into the RAM/telemetry ground truth for "did the reload path complete or stall, and where".

Usage: summarize-switch-run.py <debug-log-path>
"""
from __future__ import annotations

import re
import sys
from collections import Counter
from pathlib import Path


def main() -> int:
    if len(sys.argv) != 2:
        print(__doc__)
        return 2
    p = Path(sys.argv[1])
    if not p.exists():
        print(f"no such log: {p}")
        return 2

    disarms = 0
    switch_confirmed: set[str] = set()
    feed_slots: list[str] = []
    classes: Counter[str] = Counter()
    mms_hist: Counter[str] = Counter()
    last_switch_oracle = None
    worldres_stall = 0
    crashes: list[str] = []
    lines = 0
    ending_recovery = 0
    teardown_terminal = False
    last_wait_reason = None

    for l in p.open(encoding="utf-8", errors="replace"):
        lines += 1
        if "ENDING-REQUEST RECOVERY" in l and "SET menuData+0x5d=1" in l:
            ending_recovery += 1
        if "mms_step=-1" in l or "child left step 18" in l:
            teardown_terminal = True
        m = re.search(r"(waiting (?:for|to)[^-\n]{0,80})", l)
        if m:
            last_wait_reason = m.group(1).strip()
        if "DISARMED" in l:
            disarms += 1
        m = re.search(r"(switch #\d+/\d+ [^\n]*?(?:load )?CONFIRMED)", l)
        if m:
            switch_confirmed.add(m.group(1)[:60])
        m = re.search(r"own-load-feed:.*slot=(\d+)\) ret=1", l)
        if m:
            feed_slots.append(m.group(1))
        m = re.search(r"-- (LOADED_STABLE|DROPPED\([^)]*\)|MMS-CHILD-STALL[^\n]*?\)|bc4=1[^\n]*?\)|in-progress)", l)
        if m:
            classes[m.group(1)[:40]] += 1
        m = re.search(r"mms_step=(\d+)\(([A-Za-z_ ]+)\)", l)
        if m:
            mms_hist[f"{m.group(1)}:{m.group(2).strip()}"] += 1
        if "SWITCH-ORACLE #" in l:
            last_switch_oracle = l.strip()
        if "STEP-3 STALL DETECTED" in l or ("blk_ls=0x0" in l and "mms_step=3" in l):
            worldres_stall += 1
        for marker in ("access-violation rva", "0x1eb9999", "deliberate-abort",
                       "game ASSERT", "a0_rva=0x29c7aa0", "DL_PANIC"):
            if marker in l and "safe_input hook" not in l:
                crashes.append(l.strip()[:160])

    print(f"# {p}  ({lines} lines)")
    print(f"disarm events (FIX-1 spurious-arm):        {disarms}")
    print(f"switch CONFIRMED markers:                  {sorted(switch_confirmed) or '(none)'}")
    print(f"own-load-feed deser slots (order):         {feed_slots[:20]}  distinct={sorted(set(feed_slots))}")
    print(f"SWITCH-ORACLE class histogram:             {dict(classes)}")
    print(f"MoveMapStep-state histogram (top):         {dict(mms_hist.most_common(8))}")
    print(f"world-res-wait / step-3 null-block stalls: {worldres_stall}")
    print(f"ending-request recovery fired (SET 0x5d):  {ending_recovery}  teardown reached terminal: {teardown_terminal}")
    print(f"last 'waiting for' reason:                 {last_wait_reason or '(none)'}")
    print(f"crash/assert markers:                      {crashes[-5:] or '(none)'}")
    print(f"last SWITCH-ORACLE line:")
    print(f"  {last_switch_oracle or '(none)'}")
    # verdict heuristic
    peaked_stable = "LOADED_STABLE" in classes
    dropped = any(k.startswith("DROPPED") for k in classes)
    stuck = mms_hist.most_common(1)[0][0] if mms_hist else "n/a"
    print()
    if crashes:
        print("VERDICT: CRASH observed.")
    elif peaked_stable and not dropped:
        print("VERDICT: a load reached LOADED_STABLE (switch likely completed).")
    elif dropped:
        print("VERDICT: world DROPPED (reload bounced) -- switch did not hold.")
    elif teardown_terminal and last_wait_reason:
        print(f"VERDICT: teardown COMPLETED (child reached -1) but the clean-title reload STALLED at: '{last_wait_reason}'.")
    elif "18:" in stuck and not teardown_terminal:
        print("VERDICT: STALL at MoveMapStep 18 -- teardown never completed (ending-request lock).")
    elif "3:" in stuck:
        print("VERDICT: STALL near MoveMapStep 3 (WorldResWait).")
    else:
        print(f"VERDICT: inconclusive; most-common MoveMapStep state = {stuck}; last wait = {last_wait_reason}.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
